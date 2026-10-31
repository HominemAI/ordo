//! The `ordo` binary's guts, as library functions so they test like code.
//!
//!   ordo check      <dir|file.ordo>       every entity and op inspected
//!   ordo plan       <dir|file.ordo> [op]  the artifact JSON, all ops or one
//!   ordo verify     --space <spc>         recompute a space's hash chain
//!   ordo checkpoint --space <spc>         sign a marker at the current head
//!   ordo revert     <dir|file.ordo> …     undo a command group, as new history
//!
//! `check` is the deploy-time story in one screen, for a human: per entity
//! its typed-id key, fields, and sealed disclosures; per op its cost class,
//! row bound, and output shape — with the registration gates
//! (acknowledgments, key collisions, cycles) run first, failing loudly with
//! spans.
//!
//! `plan` is the same program for a machine. It emits the **byte-stable
//! artifact JSON** of [`ordo_plan::artifact`] — the thing a deploy diff,
//! the artifact registry and `ordo apply` consume. It used to print
//! `-- name` followed by SQL, which nothing downstream could read; the SQL
//! is still there, under each op's `plan.sql`, along with everything the
//! compiler knows that a reviewer of a deploy needs and prose cannot carry.
//!
//! Both commands take a **directory** as well as a file (P7: `ordo check
//! <dir>`), because a schema and its ops are normally more than one file.
//! See [`sources`] for the concatenation, the deterministic file order, and
//! the line map that keeps diagnostics pointing at the file the author
//! wrote.
//!
//! The last three commands are the **operator** half of the tool: they act
//! on a running deployment's database rather than on source text, and they
//! are the callers `verify_chain`, `checkpoint` and `revert_group` did not
//! have (8i). See [`operator`] for their exit codes, for what `revert`
//! checks — and, more importantly, for what it does not.

use ordo_plan::artifact::{program_artifact, to_bytes};
use ordo_schema::{build_with_keys, FieldTy, KeyTable, Schema};
use ordo_sema::Registry;
use ordo_syntax::parse_program;

pub mod deploy_wait;
pub mod dev;
pub mod dev_db;
pub mod identity;
pub mod keys;
pub mod migrate;
pub mod operator;
pub mod ops;
pub mod people;
pub mod signin;
pub mod skill;
pub mod sources;
pub mod triggers;
// 17u: the CLI talking to a deployment rather than to its database.
pub mod wire;
pub use operator::Outcome;
pub use sources::Sources;

/// Compile one anonymous source text. Spans are reported as bare
/// `line:col` — [`load_sources`] is the form that can name a file.
pub fn load(source: &str) -> Result<(Schema, Registry), String> {
    load_sources(&Sources::from_parts(vec![(
        String::new(),
        source.to_string(),
    )]))
}

/// A compile diagnostic, kept as the structured value its stage produced
/// rather than immediately flattened to a string.
///
/// [`load_sources`] still returns the flat `String` every existing caller
/// depends on — this exists so a caller that wants more than prose, namely
/// `ordo dev`'s spanned frame (`plan-dev-loop.md` D1), does not have to
/// re-parse a formatted message to recover the span `load_sources` already
/// had. `Env` is the one stage with nothing to point at: a missing or
/// malformed `ORDO_KEY_ABBREVIATIONS` is a fact about the environment, not
/// about a location in the source.
pub enum LoadDiagnostic {
    Env(String),
    Parse(ordo_syntax::ParseError),
    Schema(ordo_schema::SchemaError),
    Registration(ordo_sema::SemaError),
}

impl LoadDiagnostic {
    /// Where in the source this points, or `None` for [`LoadDiagnostic::Env`].
    pub fn span(&self) -> Option<ordo_syntax::Span> {
        match self {
            LoadDiagnostic::Env(_) => None,
            LoadDiagnostic::Parse(e) => Some(e.span),
            LoadDiagnostic::Schema(e) => Some(e.span),
            LoadDiagnostic::Registration(e) => Some(e.span),
        }
    }

    /// The closed-class code an assertion would quote, when this stage has
    /// one.
    pub fn code(&self) -> Option<&str> {
        match self {
            LoadDiagnostic::Env(_) => None,
            LoadDiagnostic::Parse(e) => Some(e.code),
            LoadDiagnostic::Schema(e) => Some(e.code),
            LoadDiagnostic::Registration(e) => Some(e.code),
        }
    }

    pub fn message(&self) -> &str {
        match self {
            LoadDiagnostic::Env(m) => m,
            LoadDiagnostic::Parse(e) => &e.message,
            LoadDiagnostic::Schema(e) => &e.message,
            LoadDiagnostic::Registration(e) => &e.message,
        }
    }

    /// The exact text [`load_sources`] has always produced for this
    /// diagnostic, given the source map that can resolve its span.
    pub fn render(&self, src: &Sources) -> String {
        match self {
            LoadDiagnostic::Env(m) => m.clone(),
            LoadDiagnostic::Parse(e) => {
                format!(
                    "{}: parse error [{}]: {}",
                    src.at(e.span),
                    e.code,
                    e.message
                )
            }
            LoadDiagnostic::Schema(e) => {
                format!(
                    "{}: schema error [{}]: {}",
                    src.at(e.span),
                    e.code,
                    e.message
                )
            }
            LoadDiagnostic::Registration(e) => format!(
                "{}: registration error [{}]: {}",
                src.at(e.span),
                e.code,
                e.message
            ),
        }
    }
}

/// Compile an assembled program, keeping each stage's own error type.
///
/// The phase-3 pipeline `plan-dev-loop.md` §1 names as the single canonical
/// sequence (`Fixture::compile_source` runs the same six calls, plus the
/// DDL a compile-only caller does not need): `parse_program` →
/// `KeyTable::from_env` → `build_with_keys` → `Registry::from_program` →
/// `acl_admin_ops` → `std_ops` → `cost::check_registration`.
pub fn load_sources_diag(src: &Sources) -> Result<(Schema, Registry), LoadDiagnostic> {
    let program = parse_program(src.text()).map_err(LoadDiagnostic::Parse)?;
    let keys = KeyTable::from_env().map_err(LoadDiagnostic::Env)?;
    let schema = build_with_keys(&program, &keys).map_err(LoadDiagnostic::Schema)?;
    // What `ordo apply` will print must be printable now. A default the DDL
    // cannot lower (`created: datetime = now`) used to pass here and fail
    // when the schema was applied, which is a program `check` called good
    // and no deployment could take (checklist 25e).
    ordo_schema::ddl_for(&schema).map_err(LoadDiagnostic::Schema)?;
    let mut registry = Registry::from_program(&program);
    // The `acl` admin ops are derived from the declaration, exactly like
    // the link entity they administer — an author writes neither. They go
    // through the same registration gate as an authored op.
    registry.extend_from(&ordo_schema::acl_admin_ops(&schema));
    // The std library (`std-library.md`), derived from the same schema by
    // the same mechanism and gated by the same call below. `ordo apply`
    // compiling std by default is the whole point of the namespace: a
    // deployment's baseline surface is a function of its shapes, not of
    // what somebody remembered to write.
    registry.extend_from(&ordo_schema::std_ops(&schema));
    ordo_plan::cost::check_registration(&schema, &registry)
        .map_err(LoadDiagnostic::Registration)?;
    Ok((schema, registry))
}

/// Compile an assembled program, reporting every span through the source
/// map so a diagnostic names the file the author wrote rather than an
/// offset into a buffer the CLI built.
pub fn load_sources(src: &Sources) -> Result<(Schema, Registry), String> {
    load_sources_diag(src).map_err(|d| d.render(src))
}

pub fn check(source: &str) -> Result<String, String> {
    check_sources(&Sources::from_parts(vec![(
        String::new(),
        source.to_string(),
    )]))
}

/// The error code an op carries at check time only because `ordo check` holds
/// no key material: a blind-index lookup's SQL needs the space's key to
/// render. The op compiles; the server renders it with the key in hand.
pub const NEEDS_KEYS_AT_RUNTIME: &str = "unresolved_blind_index";

/// Every op that does not compile, as `(op, error code)`, in the order the
/// report prints them. An op that only needs keys at runtime
/// ([`NEEDS_KEYS_AT_RUNTIME`]) is not among them.
///
/// `ordo check` exits 1 when this is non-empty (reported fix 25p): it used to
/// print `DOES NOT COMPILE` and exit 0, so a script that trusted the exit
/// code passed a broken program.
pub fn uncompiled_ops(src: &Sources) -> Result<Vec<(String, String)>, String> {
    let (schema, registry) = load_sources(src)?;
    let doc = program_artifact(&schema, &registry)?;
    Ok(doc["ops"]
        .as_array()
        .expect("`ops` is an array")
        .iter()
        .filter_map(|op| {
            let code = op["error"]["code"].as_str()?;
            (code != NEEDS_KEYS_AT_RUNTIME).then(|| {
                (
                    op["op"].as_str().unwrap_or("").to_string(),
                    code.to_string(),
                )
            })
        })
        .collect())
}

/// What `ordo check` warns about, one line each, in source order. A warning
/// never fails the check: the program means what it says, and the line
/// asks the author to say it more plainly.
///
/// Today there is one: a field or parameter typed bare `money`. It is
/// `money(0)`, whole units, so a program written before places existed
/// keeps its meaning, and new code is asked to state its places (ruled
/// 2026-10-05, `money-types` ask 1).
pub fn check_warnings(src: &Sources) -> Vec<String> {
    let mut out: Vec<String> = ordo_syntax::bare_money_sites(src.text())
        .into_iter()
        .map(|span| {
            format!(
                "warning: {}: bare `money` is `money(0)`, whole units; state the places, \
                 `money(2)` for cents or `money(0)` for whole units",
                src.at(span)
            )
        })
        .collect();
    if let Ok((schema, registry)) = load_sources(src) {
        out.extend(index_warnings(&schema, &registry));
    }
    out
}

/// Does an index on `entity` lead with `column`? The primary key leads with
/// `id`; a `unique` field, a composite `unique (…)`, a declared `index (…)`
/// and the index every ref gets ([`ordo_schema::Entity::all_indexes`]) lead
/// with their first field. Every one of them sits behind `$space`, which
/// every statement fixes.
fn index_leads(e: &ordo_schema::Entity, column: &str) -> bool {
    let Some(field) = e.fields.iter().find(|f| f.column() == column) else {
        return false;
    };
    field.name == "id"
        || field.unique
        || e.all_indexes()
            .iter()
            .chain(&e.unique_keys)
            .any(|k| k.first() == Some(&field.name))
}

/// The guards and the list sorts no index can serve (board
/// `guard-equality-uses-no-index`). One line each, naming the ops.
///
/// - **A guard's walk.** A guard is a semi-join run once per row it checks.
///   When none of the columns it compares with `==` on the walked table has
///   an index leading with it, each check scans that table: the shape of the
///   audit's 450 ms `myNotes`. One indexed column is enough; Postgres
///   narrows by it and filters the rest.
/// - **A list's sort key.** A list ordered by a column no index leads with
///   is sorted whole on every read, and on every page of a sync.
///
/// Authored ops only: the std library's ops are derived from the same
/// shapes and would repeat each line. A warning never fails the check.
pub fn index_warnings(schema: &Schema, registry: &Registry) -> Vec<String> {
    use ordo_sema::ir::{BCond, BExpr, From, Query};
    use std::collections::BTreeMap;

    fn conds<'a>(c: &'a BCond, out: &mut Vec<&'a BCond>) {
        match c {
            BCond::And(xs) | BCond::Or(xs) => xs.iter().for_each(|x| conds(x, out)),
            BCond::Not(x) => conds(x, out),
            other => out.push(other),
        }
    }
    /// For every semi-join under `q` over a table: the table, and the
    /// columns of it compared with `==` against something outside its row.
    fn walks(q: &Query, out: &mut Vec<(String, Vec<String>)>) {
        let mut atoms = Vec::new();
        for c in q
            .filters
            .iter()
            .chain(q.joins.iter().filter_map(|j| j.guard.as_ref()))
        {
            conds(c, &mut atoms);
        }
        for a in atoms {
            let BCond::Exists { query, .. } = a else {
                continue;
            };
            if let From::Table { entity, alias } = &query.from {
                let own = |e: &BExpr| match e {
                    BExpr::Col {
                        alias: a, column, ..
                    } if a == alias => Some(column.clone()),
                    _ => None,
                };
                let mut inner = Vec::new();
                query.filters.iter().for_each(|c| conds(c, &mut inner));
                let mut cols = Vec::new();
                for c in inner {
                    if let BCond::Cmp {
                        op: ordo_syntax::CmpOp::Eq,
                        lhs,
                        rhs,
                    } = c
                    {
                        if let (Some(col), None) | (None, Some(col)) = (own(lhs), own(rhs)) {
                            cols.push(col);
                        }
                    }
                }
                if !cols.is_empty() {
                    out.push((entity.clone(), cols));
                }
            }
            walks(query, out);
        }
    }
    fn ops(names: &[String]) -> String {
        let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
        match quoted.as_slice() {
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
            [] => String::new(),
        }
    }

    let mut names: Vec<&String> = registry
        .names()
        .filter(|n| !n.starts_with("std."))
        .collect();
    names.sort();
    // (entity, columns) → the ops that reach it.
    let mut unserved_walks: BTreeMap<(String, Vec<String>), Vec<String>> = BTreeMap::new();
    let mut unserved_sorts: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for name in names {
        let Ok(q) = ordo_sema::bind_op_generic(schema, registry, name, ordo_sema::Clock::CORPUS)
        else {
            continue;
        };
        let mut found = Vec::new();
        walks(&q, &mut found);
        for (entity, cols) in found {
            let Some(e) = schema.entity(&entity) else {
                continue;
            };
            if !cols.iter().any(|c| index_leads(e, c)) {
                let ops = unserved_walks.entry((entity, cols)).or_default();
                if !ops.contains(name) {
                    ops.push(name.clone());
                }
            }
        }
        if let From::Table { entity, alias } = &q.from {
            let Some(e) = schema.entity(entity) else {
                continue;
            };
            for s in &q.sort {
                if let BExpr::Col {
                    alias: a, column, ..
                } = &s.expr
                {
                    if a == alias && !index_leads(e, column) {
                        let ops = unserved_sorts
                            .entry((entity.clone(), column.clone()))
                            .or_default();
                        if !ops.contains(name) {
                            ops.push(name.clone());
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for ((entity, cols), names) in unserved_walks {
        let listed: Vec<String> = cols.iter().map(|c| format!("`{entity}.{c}`")).collect();
        // Suggest a column an index can hold: not a sealed one.
        let sealed = |c: &String| {
            schema
                .entity(&entity)
                .and_then(|e| e.fields.iter().find(|f| &f.column() == c))
                .is_some_and(|f| f.sealed)
        };
        let suggest = cols.iter().find(|c| !sealed(c)).unwrap_or(&cols[0]);
        out.push(format!(
            "warning: {} {} through a guard that compares {}, and no index leads with \
             any of them: each row checked scans `{entity}`. Mark `{entity}.{}` \
             `index`.",
            ops(&names),
            if names.len() == 1 { "reads" } else { "read" },
            listed.join(" and "),
            suggest
        ));
    }
    for ((entity, column), names) in unserved_sorts {
        out.push(format!(
            "warning: {} {} `{entity}` by `{column}`, and no index leads with it: every \
             read sorts the whole list. Mark `{entity}.{column}` `index`.",
            ops(&names),
            if names.len() == 1 { "sorts" } else { "sort" },
        ));
    }
    out
}

pub fn check_sources(src: &Sources) -> Result<String, String> {
    let (schema, registry) = load_sources(src)?;
    let mut out = String::new();

    // Name what was checked, and in what order, whenever it was more than
    // one file. A reviewer who cannot see the input set cannot tell a
    // green check of the whole program from a green check of half of it.
    let labels = src.labels();
    if labels.len() > 1 {
        out.push_str(&format!("SOURCES ({})\n", labels.len()));
        for l in &labels {
            out.push_str(&format!("  {l}\n"));
        }
        out.push('\n');
    }

    if let Some(d) = &schema.domain {
        out.push_str(&format!("domain {d}\n\n"));
    }

    out.push_str("ENTITIES\n");
    for e in &schema.entities {
        out.push_str(&format!("  {}  (id key: {}_)\n", e.name, e.key));
        for f in e.fields.iter().filter(|f| !f.implicit) {
            let ty = match &f.ty {
                // A field pinned to a closed set prints as the set, not as
                // `text`. It is derived rather than declared today, so the
                // shape summary is the only place a reader can learn which
                // values `level` admits — printing `text` would hide the
                // whole ladder the acl ordering is built on (GC036).
                FieldTy::Scalar(_) if f.enum_values.is_some() => format!(
                    "enum({})",
                    f.enum_values
                        .as_ref()
                        .expect("checked")
                        .iter()
                        .map(|v| format!("\"{v}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                // `text(64)` prints with its bound: the number is a
                // constraint the database and the door both keep, so the
                // shape summary states it beside the type it bounds.
                FieldTy::Scalar(s) => match f.text_max {
                    Some(n) => format!("{}({n})", s.as_str()),
                    None => s.as_str().to_string(),
                },
                FieldTy::Ref {
                    target,
                    owned,
                    tree,
                } => format!(
                    "-> {target}{}{}",
                    if *owned { " owned" } else { "" },
                    if *tree { " tree" } else { "" }
                ),
            };
            let mut marks = Vec::new();
            if f.optional {
                marks.push("nullable");
            }
            if f.unique {
                marks.push("unique");
            }
            if f.sealed {
                marks.push(if f.indexed {
                    "sealed+indexed (equality via blind index; value concealed, nullness is not)"
                } else {
                    "sealed (opened by the host, one key release per page)"
                });
            }
            out.push_str(&format!(
                "    {}: {ty}{}\n",
                f.name,
                if marks.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", marks.join(", "))
                }
            ));
        }
        // Composite unique keys: a field-level `[unique]` mark cannot say
        // "these two together", and R-G17's converge-or-create grant is
        // built on exactly that.
        for key in &e.unique_keys {
            out.push_str(&format!("    unique ({})\n", key.join(", ")));
        }
        if e.is_tree {
            out.push_str("    (tree: implicit depth; under/above via stored path)\n");
        }
        // DL §3: `std sealed` is a shape line so that it shows in the shape
        // summary. Without it a sealed field is absent from the derived std
        // ops; with it they disclose the whole row.
        if e.std_sealed {
            out.push_str("    std sealed — derived std ops include the sealed fields\n");
        }
        // The acl declaration and the bootstrap it implies, stated where a
        // reviewer reads the shape rather than left to the docs (GC091).
        if let Some(levels) = &e.acl_levels {
            out.push_str(&format!(
                "    acl ({}) — creator receives {} at create, space members only\n",
                levels.join(", "),
                levels.last().map(String::as_str).unwrap_or("—"),
            ));
        }
        match &e.guards {
            None => out.push_str("    guards: NONE — unguarded\n"),
            Some(g) => {
                let prov = match &g.source {
                    ordo_schema::GuardSource::Own => "own".to_string(),
                    ordo_schema::GuardSource::Inherited { from } => {
                        format!("inherited from {from}")
                    }
                    ordo_schema::GuardSource::AclDerived => "acl-derived".to_string(),
                };
                out.push_str(&format!(
                    "    guards ({prov}):\n      read   {}\n      write  {}\n",
                    g.render(true),
                    g.render(false)
                ));
            }
        }
    }

    // The op report is a **rendering of the artifact**, not a second walk
    // of the schema and registry. Two renderings from one source is the
    // pattern the catalog already uses (machine JSON and prose), and it is
    // the only arrangement in which `ordo check` and `ordo plan` cannot
    // disagree about a cost class, a mode label or a guard.
    //
    // It also fixes what this loop used to do: it planned every op and
    // `?`-ed out on the first failure, so one op whose SQL cannot be
    // produced without key material — any blind-index op, and the market
    // fixture has one — took the whole report down and printed nothing
    // about the other fifty-seven.
    // Whether a member may leave on their own (27aq). Printed with no rule
    // too, because "members cannot leave" is the answer a developer goes
    // hunting for when nobody can (owner, 2026-10-05, ask 4).
    out.push_str("\nRELEASE\n");
    match &schema.release {
        Some(r) => {
            for line in ordo_schema::render_release(r).lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
        None => out.push_str(
            "  no release rule: members cannot leave on their own (locked); \
             see docs/howto-leave.md\n",
        ),
    }

    // Scheduled and on-change work (checklist 12a). Not ops: nobody calls
    // them, so they are listed apart, with what wakes each one.
    if !schema.routines.is_empty() {
        out.push_str("\nROUTINES\n");
        for r in &schema.routines {
            out.push_str(&format!("  {}\n", ordo_host::routines::describe_one(r)));
        }
    }

    out.push_str("\nOPS\n");
    let doc = program_artifact(&schema, &registry)?;
    for op in doc["ops"].as_array().expect("`ops` is an array") {
        let s = |k: &str| op[k].as_str().unwrap_or("").to_string();
        let name = s("op");
        out.push_str(&format!(
            "  {name}{}{}{}{}{}\n",
            if op["flags"]["unbounded"] == true {
                "  (unbounded)"
            } else {
                ""
            },
            if op["flags"]["unguarded"] == true {
                "  (unguarded)"
            } else {
                ""
            },
            // 27ar: this op keeps answering while its tenant is paused.
            if op["flags"]["nopause"] == true {
                "  (nopause: answers while the tenant is paused)"
            } else {
                ""
            },
            // A visitor with no token may call it (`design-public-path.md`).
            if op["flags"]["public"] == true {
                "  (public: anyone may call it, with no token)"
            } else {
                ""
            },
            // Its rows go into a person's own download (`GET /download`).
            if op["flags"]["download"] == true {
                "  (download: in a person's own download)"
            } else {
                ""
            },
        ));
        if let Some(e) = op["error"].as_object() {
            out.push_str(&format!(
                "    DOES NOT COMPILE [{}] {}\n",
                e["code"].as_str().unwrap_or(""),
                e["message"].as_str().unwrap_or("")
            ));
            continue;
        }
        match s("kind").as_str() {
            "read" => {
                let shape: Vec<String> = op["outputs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| {
                        format!(
                            "{}: {}{}",
                            o["name"].as_str().unwrap_or(""),
                            o["type"].as_str().unwrap_or(""),
                            if o["sealed"] == true { " (sealed)" } else { "" }
                        )
                    })
                    .collect();
                out.push_str(&format!(
                    "    class {}   rows {}\n    [{}]\n",
                    op["cost"]["class"].as_str().unwrap_or(""),
                    match op["cost"]["row_bound"].as_i64() {
                        Some(n) => format!("<= {n}"),
                        None => "unbounded (pageable)".to_string(),
                    },
                    shape.join(", ")
                ));
                if let Some(line) = refinable_line(op, &doc) {
                    out.push_str(&format!("    {line}\n"));
                }
            }
            kind => out.push_str(&format!("    {kind}   mode {}\n", s("mode"))),
        }
        // The field-scope manifest (9l): a reviewer reading `ordo check`
        // sees which FIELDS the op reaches, including the ones only a
        // guard reads — the same list the catalog and the consent page
        // render, from the same artifact.
        for e in op["scope"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "    reaches {}\n",
                ordo_plan::artifact::render_scope_json(e)
            ));
        }
        if let Some(p) = op["release"]["purpose"].as_str() {
            let keys: Vec<String> = op["release"]["keys"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|k| {
                    format!(
                        "{}.{} ({})",
                        k["entity"].as_str().unwrap_or(""),
                        k["field"].as_str().unwrap_or(""),
                        k["key"].as_str().unwrap_or(""),
                    )
                })
                .collect();
            out.push_str(&format!("    releases [{p}] {}\n", keys.join(", ")));
        }
        if let Some(b) = op["bypassed_guards"].as_array() {
            if !b.is_empty() {
                let names: Vec<&str> = b.iter().filter_map(|v| v.as_str()).collect();
                out.push_str(&format!("    BYPASSES {}\n", names.join(", ")));
            }
        }
    }
    Ok(out)
}

/// One read's `refinable:` line: the fields a refinement's condition may
/// name, grouped by what each allows (ruled 2026-10-05,
/// `refinement-default` ask 4: print, do not guess).
///
/// ```text
/// refinable: placed, total, $id (any operator); memo (refused)
/// ```
///
/// Computed from the program alone, by the function that writes the
/// descriptor's `ops[].refine` ([`ordo_plan::interface::refinable_fields`]),
/// so `ordo check` and `interface.json` cannot disagree. It says what the
/// program allows. What a request is refused for at run time (the size
/// limits, the cost gate on an unbounded op) is `docs/howto-cost.md`'s. A
/// read with no refinable surface prints nothing. It never fails the check.
pub fn refinable_line(op: &serde_json::Value, doc: &serde_json::Value) -> Option<String> {
    use ordo_plan::interface::{refinable_fields, FieldRefine};
    let empty = Vec::new();
    let ops = doc["ops"].as_array().unwrap_or(&empty);
    let entities = doc["entities"].as_array().unwrap_or(&empty);
    let fields = refinable_fields(op, entities, ops);
    if fields.is_empty() {
        return None;
    }
    let groups: Vec<String> = [
        FieldRefine::Any,
        FieldRefine::Equality,
        FieldRefine::Refused,
    ]
    .into_iter()
    .filter_map(|allows| {
        let names: Vec<&str> = fields
            .iter()
            .filter(|(_, a)| *a == allows)
            .map(|(n, _)| n.as_str())
            .collect();
        (!names.is_empty()).then(|| format!("{} ({})", names.join(", "), allows.label()))
    })
    .collect();
    Some(format!("refinable: {}", groups.join("; ")))
}

/// `ordo plan` — the artifact JSON, byte-stable, for one op or all.
///
/// Filtering keeps the whole document shape (`domain`, `entities`,
/// `ops`) and narrows only `ops`, so a single-op artifact is a subsequence
/// of the full one rather than a second format: whatever reads one reads
/// the other, and `ordo plan <dir> <op>` is a debugging convenience rather
/// than a fork in the contract.
pub fn plan_ops(source: &str, only: Option<&str>) -> Result<String, String> {
    plan_artifact(
        &Sources::from_parts(vec![(String::new(), source.to_string())]),
        only,
    )
}

pub fn plan_artifact(src: &Sources, only: Option<&str>) -> Result<String, String> {
    let (schema, registry) = load_sources(src)?;
    let mut doc = program_artifact(&schema, &registry)?;
    if let Some(o) = only {
        let kept: Vec<serde_json::Value> = doc["ops"]
            .as_array()
            .expect("`ops` is an array")
            .iter()
            .filter(|op| op["op"] == *o)
            .cloned()
            .collect();
        if kept.is_empty() {
            return Err(format!("no op named `{o}`"));
        }
        doc["ops"] = serde_json::Value::Array(kept);
    }
    Ok(to_bytes(&doc))
}

/// `ordo interface` — the **Interface Descriptor** (`interface.json`), the
/// one data contract the generated SDK, the LSP and generated UI share
/// (`tooling-and-embedding-plan.md` §B1).
///
/// It is a projection of the artifact this same binary emits: the entity
/// half straight from [`ordo_plan::artifact::entity_artifacts`], the op half
/// through [`ordo_plan::artifact::catalog_view`] — the catalog manifest, so
/// that the document a compile step writes and the document a deployment
/// serves per asker are the same document with a different op list.
///
/// **Unscoped, deliberately.** This is the compile-only tier: there is no
/// token and no access engine at a build step, so the file describes the
/// whole program. Scoping is the serving deployment's act
/// (`ordo_host::interface_for`), and it is the same function with the ops
/// filtered — never a second, looser descriptor that a build step could
/// accidentally publish.
pub fn interface_json(src: &Sources) -> Result<String, String> {
    let (schema, registry) = load_sources(src)?;
    let doc = program_artifact(&schema, &registry)?;
    Ok(to_bytes(&interface_value(&schema, &doc)))
}

/// The descriptor as a value, from a compiled schema and its artifact. The
/// one projection `ordo interface` prints and `ordo ops` reads, so the two
/// cannot describe different programs.
pub fn interface_value(schema: &Schema, artifact: &serde_json::Value) -> serde_json::Value {
    let ops: Vec<serde_json::Value> = artifact["ops"]
        .as_array()
        .expect("`ops` is an array")
        .iter()
        .map(ordo_plan::artifact::catalog_view)
        .collect();
    ordo_plan::interface::interface_document(
        &artifact["domain"],
        &ordo_plan::artifact::entity_artifacts(schema),
        &ops,
    )
}

/// `ordo gen ts <dir|file.ordo> --out <root>` — regenerate both TypeScript
/// packages' generated halves.
///
/// The one command the generator-first architecture rests on. Both
/// `clients/ordo-sdk` and `clients/ordo-cdk` are **generated from the
/// implementation's own canonical tables** — the SDK from this program's
/// interface descriptor, the CDK's language layer from the parser's closed
/// vocabularies and its schema layer from the same descriptor — so language
/// evolution is this command, not an editing session in two packages.
///
/// It writes under `root`, which is the repository root in the ordinary case.
/// The paths are the generator's (`ordo_gen::SDK_OUT`, `ordo_gen::CDK_OUT`)
/// rather than this function's, because the golden test that checks the
/// checked-in files against a fresh run has to name the same places and a
/// second list of paths is a second answer.
pub fn generate_ts(src: &Sources, root: &std::path::Path) -> Result<String, String> {
    let json = interface_json(src)?;
    let doc: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("the descriptor is not JSON: {e}"))?;
    let files = ordo_gen::generate(&doc)?;
    let mut report = String::new();
    for f in &files {
        let path = root.join(&f.path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, &f.contents).map_err(|e| format!("{}: {e}", path.display()))?;
        report.push_str(&format!("{}\n", f.path));
    }
    Ok(report)
}

/// The package an app imports the SDK's hand-written core from.
pub const SDK_PACKAGE_CORE: &str = "@hominemai/ordo-sdk/core/";

/// `ordo gen ts <dir> --app <srcdir>`: the typed client for one app.
///
/// Writes `<srcdir>/generated/{ids,specs,ops,index}.ts` with their core
/// imports pointed at the installed package, `<srcdir>/interface.json` for
/// the React hooks, and `<srcdir>/generated/OPS.md`, the `ordo ops` index
/// with a generated header. This is what every app's `gen.sh` did by hand
/// with a scratch directory and `sed`. `path` is the program path as the
/// caller spelled it, for the commands OPS.md names.
pub fn generate_app(src: &Sources, path: &str, srcdir: &std::path::Path) -> Result<String, String> {
    let json = interface_json(src)?;
    let doc: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("the descriptor is not JSON: {e}"))?;
    let gen_dir = srcdir.join("generated");
    std::fs::create_dir_all(&gen_dir).map_err(|e| format!("{}: {e}", gen_dir.display()))?;
    let mut written: Vec<(std::path::PathBuf, String)> = ordo_gen::sdk::files(&doc, "generated")?
        .into_iter()
        .map(|f| {
            (
                srcdir.join(&f.path),
                f.contents
                    .replace("'../core/", &format!("'{SDK_PACKAGE_CORE}")),
            )
        })
        .collect();
    written.push((srcdir.join("interface.json"), json));
    let md = ops::ops_md(src, path).map_err(|e| e.to_string())?;
    written.push((gen_dir.join("OPS.md"), md));
    let mut report = String::new();
    for (p, text) in &written {
        std::fs::write(p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        report.push_str(&format!("{}\n", p.display()));
    }
    Ok(report)
}
