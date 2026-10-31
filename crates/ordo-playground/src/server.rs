//! The playground's request handlers, as ordinary functions.
//!
//! They used to live in `main.rs`, which made 375 lines of the crate's 1267
//! unreachable from any test: a binary has no library surface, so nothing
//! could ask what `/api/forward` answers. The axum layer is now a five-line
//! wrapper per route and every answer is a plain `serde_json::Value` a test
//! can assert on.
//!
//! Nothing here is async except the wrappers. The handlers shell out to
//! `psql` through `ordo_conformance::Pg` — blocking work either way — and a
//! synchronous surface is one a test can call without a runtime.

use serde::Deserialize;
use serde_json::{json, Value};

use ordo_conformance::{Fixture, Pg};

use crate::decompile;

/// The date every `since` window in the playground anchors to.
///
/// `since` is specified against *execution* time, so the honest default
/// would be [`ordo_sema::Clock::now`] — but the fixtures are seeded around
/// 2026-04/05 and a real clock makes every window on them empty, which
/// teaches the reader the wrong thing about a feature that works. So the
/// playground pins the corpus date, and **says which date it pinned**: the
/// forward response carries `clock` and the page prints it. A pinned clock
/// nobody is told about is the same bug the binder's `Clock` argument was
/// introduced to prevent, moved one layer out.
pub const CLOCK: ordo_sema::Clock = ordo_sema::Clock::CORPUS;

/// Reads are served from the playground's OWN databases.
///
/// They were served from `ordo_conformance` and `ordo_wide` — the
/// conformance suites' databases, which the playground never seeds. Run the
/// playground on a tree where `cargo test` has not run and every read
/// answered `relation "orders" does not exist`; run it *while* `cargo test`
/// runs and reads raced a `DROP SCHEMA public CASCADE`.
pub const MARKET_DB: &str = "ordo_playground_market";
pub const WIDE_DB: &str = "ordo_playground_wide";
/// Writes land here, never in the read databases: a mutated read pane is a
/// playground that answers differently on Tuesday.
pub const WRITE_DB: &str = "ordo_playground";

pub struct App {
    pub market: Fixture,
    pub wide: Fixture,
    pub writes: Fixture,
    pub log: std::sync::Mutex<ordo_conformance::WriteLog>,
    /// Which databases this instance reads and writes. Fields rather than
    /// constants so a test binary can take three of its own and not race
    /// the playground a developer has running beside it.
    pub market_db: String,
    pub wide_db: String,
    pub write_db: String,
}

impl App {
    /// Compile the fixtures. Touches no database.
    pub fn load() -> Result<App, String> {
        Ok(App {
            market: Fixture::load()?,
            wide: Fixture::load_wide()?,
            writes: Fixture::load()?,
            log: std::sync::Mutex::new(ordo_conformance::WriteLog::new()),
            market_db: MARKET_DB.into(),
            wide_db: WIDE_DB.into(),
            write_db: WRITE_DB.into(),
        })
    }

    /// The same fixtures over a private set of databases, named by prefix.
    pub fn with_db_prefix(mut self, prefix: &str) -> App {
        self.market_db = format!("{prefix}_market");
        self.wide_db = format!("{prefix}_wide");
        self.write_db = format!("{prefix}_writes");
        self
    }

    /// Build and seed all three databases. Reported per database rather than
    /// as one boolean: a playground whose write pane is down but whose read
    /// panes work should say exactly that.
    pub fn seed(&self) -> Vec<(String, Result<(), String>)> {
        vec![
            (
                self.market_db.clone(),
                self.market.reset(&Pg::named(&self.market_db)),
            ),
            (
                self.wide_db.clone(),
                self.wide.reset(&Pg::named(&self.wide_db)),
            ),
            (
                self.write_db.clone(),
                self.writes.reset(&Pg::named(&self.write_db)),
            ),
        ]
    }

    pub fn fixture(&self, name: &str) -> &Fixture {
        if name == "wide" {
            &self.wide
        } else {
            &self.market
        }
    }

    pub fn pg(&self, name: &str) -> Pg {
        Pg::named(if name == "wide" {
            &self.wide_db
        } else {
            &self.market_db
        })
    }
}

#[derive(Deserialize, Default)]
pub struct ForwardReq {
    pub fixture: String,
    pub source: String,
    #[serde(default)]
    pub run: bool,
    /// Whose eyes: a member id, or absent for anonymous (every guard
    /// false — a defined rule, not an accident).
    #[serde(default)]
    pub caller: Option<String>,
    /// Rows per page; absent runs unpaged.
    #[serde(default)]
    pub page_size: Option<i64>,
    /// The previous page's cursor keys, as returned.
    #[serde(default)]
    pub after: Option<Vec<Option<String>>>,
}

#[derive(Deserialize)]
pub struct WriteReq {
    /// A write op call, e.g. `renameTask($task: "task_1", $title: "x")`.
    pub call: String,
    pub caller: String,
}

#[derive(Deserialize)]
pub struct ManifestReq {
    pub fixture: String,
    pub op: String,
}

#[derive(Deserialize, Default)]
pub struct ReverseReq {
    pub fixture: String,
    pub sql: String,
    #[serde(default)]
    pub run: bool,
}

pub fn forward(app: &App, req: &ForwardReq) -> Value {
    let f = app.fixture(&req.fixture);
    // `whole entity("id")` — the anchored composition read.
    if req.source.trim_start().starts_with("whole ") {
        // As the chosen member — the selector governs `whole` exactly as it
        // governs every other read. (It did not: `whole` ignored the
        // caller entirely, which was the visible half of a guard bypass.)
        let caller = req.caller.as_deref().filter(|c| !c.is_empty());
        return match f.run_whole_as(&app.pg(&req.fixture), req.source.trim(), caller) {
            Ok(doc) => json!({ "whole": doc }),
            Err(e) => json!({ "error": e }),
        };
    }
    let bound_info = f.bind_case(&req.source).ok().map(|q| {
        (
            ordo_plan::cost::classify_with(&q, Some(&f.schema))
                .as_str()
                .to_string(),
            ordo_plan::cost::row_bound(&q, &f.schema).as_string(),
        )
    });
    match f.compile(&req.source) {
        Err(e) => json!({ "error": e }),
        Ok(plan) => {
            let (y, m, d) = CLOCK.0;
            let mut out = json!({
                "cost": bound_info.as_ref().map(|(c, _)| c.clone()),
                "rows_bound": bound_info.as_ref().map(|(_, b)| b.clone()),
                "clock": format!("{y:04}-{m:02}-{d:02}"),
                "sql": plan.sql,
                "outputs": plan.outputs.iter()
                    .map(|(n, t)| format!("{n}: {}", t.as_str()))
                    .collect::<Vec<_>>(),
            });
            if req.run {
                let pg = app.pg(&req.fixture);
                let caller = req.caller.as_deref().filter(|c| !c.is_empty());
                match req.page_size {
                    // Paged: the SQL shown above is the unpaged plan; what
                    // ran is its keyset page, and the cursor comes back so
                    // the page can ask for the next one.
                    Some(n) if n > 0 => {
                        match paged_rows(f, &pg, &req.source, caller, n, req.after.as_deref()) {
                            Ok((rows, next)) => {
                                out["rows"] = rows;
                                out["next"] = json!(next);
                            }
                            Err(e) => out["run_error"] = json!(e),
                        }
                    }
                    _ => match f.run_case_as(&pg, &req.source, &[], caller) {
                        Ok(rows) => out["rows"] = door_spelling(rows, &plan.outputs),
                        Err(e) => out["run_error"] = json!(e),
                    },
                }
            }
            out
        }
    }
}

/// A page's rows and the cursor keys addressing the row after them.
pub type PageResult = (Value, Option<Vec<Option<String>>>);

/// One keyset page, planned and run through the harness. (The playground
/// does not hold a pooled executor — it is a single-user page — so it uses
/// the same planner entry point the executor does and runs the SQL here.)
pub fn paged_rows(
    f: &Fixture,
    pg: &Pg,
    source: &str,
    caller: Option<&str>,
    n: i64,
    after: Option<&[Option<String>]>,
) -> Result<PageResult, String> {
    let pl = ordo_syntax::parse_pipeline(source)
        .map_err(|e| format!("parse: {} at {}", e.message, e.span))?;
    let mut q = ordo_sema::bind_as(&f.schema, &f.registry, &pl, &[], caller, CLOCK)
        .map_err(|e| format!("bind: {e}"))?;
    // The step the unpaged path gets from `Fixture::compile` and this one
    // did not: without it any query touching a `sealed indexed` field
    // refused with `unresolved_blind_index` the moment a page size was set,
    // while the same query ran unpaged. Same pipeline, two answers.
    f.resolve_blind(&mut q, &[])?;
    let paged =
        ordo_plan::plan_paged(&q, &f.plan_space(), n, after).map_err(|e| format!("plan: {e}"))?;
    let raw = pg.query(&paged.sql)?;
    let full = raw.len() as i64 == n;
    let names = ordo_plan::order_key_outputs(&q);
    let next = if full {
        raw.last().map(|r| {
            let outs = paged.outputs.len();
            let mut hidden = r[outs..].iter().cloned();
            names
                .iter()
                .map(|name| match name {
                    Some(n) => paged
                        .outputs
                        .iter()
                        .position(|(o, _)| o == n)
                        .and_then(|p| r.get(p).cloned())
                        .flatten(),
                    None => hidden.next().flatten(),
                })
                .collect()
        })
    } else {
        None
    };
    let trimmed: Vec<Vec<Option<String>>> = raw
        .into_iter()
        .map(|mut r| {
            r.truncate(paged.outputs.len());
            r
        })
        .collect();
    let rows = ordo_conformance::canonical(&trimmed, &paged.outputs);
    Ok((door_spelling(rows, &paged.outputs), next))
}

/// Rows as the door spells them.
///
/// The harness decodes money as stored minor units (`2999`), because its
/// corpus asserts what the database holds. The door answers a `money(p)`
/// as a string in major units with exactly `p` places (`"29.99"`). The
/// playground shows what a client would receive, so it respells each money
/// output with the door's own function. Decimals already arrive as their
/// exact string, and every other type is left as it is.
pub fn door_spelling(rows: Value, outputs: &[(String, ordo_syntax::ScalarTy)]) -> Value {
    let Value::Array(rows) = rows else {
        return rows;
    };
    let money: Vec<(&str, u8)> = outputs
        .iter()
        .filter_map(|(name, ty)| match ty {
            ordo_syntax::ScalarTy::Money(p) => Some((name.as_str(), *p)),
            _ => None,
        })
        .collect();
    Value::Array(
        rows.into_iter()
            .map(|mut row| {
                if let Value::Object(obj) = &mut row {
                    for (name, places) in &money {
                        if let Some(v) = obj.get_mut(*name) {
                            if let Some(minor) = v.as_i64() {
                                *v = json!(ordo_syntax::numeric::major_spelling(
                                    i128::from(minor),
                                    *places
                                ));
                            }
                        }
                    }
                }
                row
            })
            .collect(),
    )
}

/// Run a write op as a member: the real Stage-5 protocol, against the
/// playground's own database, returning the row summary AND the command
/// group it appended — before/after images included, because that is the
/// half of a write nobody usually gets to see.
pub fn write(app: &App, req: &WriteReq) -> Value {
    use ordo_commandlog::store::Store;
    let pg = Pg::named(&app.write_db);
    let mut log = app.log.lock().expect("log");
    let before_len = log.len();
    let result = app
        .writes
        .run_write_as(&pg, &mut log, req.call.trim(), Some(&req.caller));
    match result {
        Err(e) => json!({ "error": e }),
        Ok(summary) => {
            let space = ordo_commandlog::SpaceId("conformance".into());
            let group = log.store.get(&space, before_len).map(|g| {
                json!({
                    "correlation": format!("{:?}", g.group.correlation),
                    "actor": format!("{:?}", g.group.actor),
                    "commands": g.group.commands.iter().map(|c| json!({
                        "entity": c.entity,
                        "row": c.row_id,
                        "kind": format!("{:?}", c.kind),
                        "before": c.before.as_ref().map(image),
                        "after": c.after.as_ref().map(image),
                    })).collect::<Vec<_>>(),
                })
            });
            json!({ "summary": summary, "group": group })
        }
    }
}

fn image(i: &ordo_commandlog::Image) -> Value {
    Value::Object(
        i.iter()
            .map(|(k, v)| {
                let rendered = match v {
                    ordo_commandlog::Value::Null => json!(null),
                    ordo_commandlog::Value::Bool(b) => json!(b),
                    ordo_commandlog::Value::Int(n) => json!(n),
                    ordo_commandlog::Value::Text(t) | ordo_commandlog::Value::Id(t) => json!(t),
                    // Sealed values never appear in an image; the handle does.
                    ordo_commandlog::Value::SealedRef(_) => json!("<sealed>"),
                    ordo_commandlog::Value::Decimal(d) => json!(d.to_string()),
                };
                (k.clone(), rendered)
            })
            .collect(),
    )
}

/// What the catalog publishes for one op.
pub fn manifest(app: &App, req: &ManifestReq) -> Value {
    let f = app.fixture(&req.fixture);
    match ordo_host::manifest(f, &req.op) {
        Some(m) => m,
        None => json!({ "error": format!("no registered op `{}`", req.op) }),
    }
}

/// Rows as a comparable multiset, from either shape the harness returns:
/// `run_sql` yields arrays, `run_case` yields objects. Values are sorted
/// within a row because the two sides may order columns differently and the
/// question being asked is "the same rows?", not "the same layout?".
pub fn row_multiset(rows: &Value) -> Vec<String> {
    let one = |r: &Value| -> String {
        let mut vals: Vec<String> = match r {
            Value::Array(a) => a.iter().map(|x| x.to_string()).collect(),
            Value::Object(o) => o.values().map(|x| x.to_string()).collect(),
            other => vec![other.to_string()],
        };
        vals.sort();
        vals.join("|")
    };
    let mut out: Vec<String> = rows
        .as_array()
        .map(|a| a.iter().map(one).collect())
        .unwrap_or_default();
    out.sort();
    out
}

pub fn reverse(app: &App, req: &ReverseReq) -> Value {
    let f = app.fixture(&req.fixture);
    // The most common misfire on this pane is pasting ORDO into the SQL box
    // — the tokenizer then reports something true and useless ("expected an
    // identifier, found Str(..)"). Recognise the shape and say the helpful
    // thing instead. The test is deliberately conservative: no SELECT/WITH
    // anywhere, and a construct only Ordo spells.
    {
        let t = req.sql.trim();
        let lower = t.to_lowercase();
        let has_sql_head =
            lower.contains("select") || lower.starts_with("with") || lower.starts_with("(");
        let ordo_marks = t.starts_with("op ")
            || t.starts_with("entity ")
            || t.starts_with("whole ")
            || t.contains("\nappend ")
            || t.starts_with("append ")
            || t.contains("\ntake ")
            || t.contains("\nsort ");
        if !has_sql_head && ordo_marks {
            return json!({
                "error": "this looks like Ordo, not SQL — this pane translates \
                          SQL into Ordo; paste Ordo source into the forward pane \
                          on the left"
            });
        }
    }
    match decompile::decompile(&f.schema, &req.sql) {
        Err(r) => json!({ "error": format!("outside the v0 subset: {}", r.what) }),
        Ok(d) => {
            // Verify: the emitted Ordo must compile forward. Shown, never
            // assumed.
            let mut out = json!({ "ordo": d.ordo, "notes": d.notes });
            match f.compile(&d.ordo) {
                Err(e) => {
                    out["verify_error"] =
                        json!(format!("the decompilation does not compile back: {e}"))
                }
                Ok(plan) => {
                    out["roundtrip_sql"] = json!(plan.sql);
                    if req.run {
                        let pg = app.pg(&req.fixture);
                        // BOTH sides. The page claimed a divergence would be
                        // "visible, not hidden" while showing only the
                        // decompilation's rows — so the one thing a reader
                        // needed in order to disbelieve the translation was
                        // the one thing not on the page.
                        let ordo_rows = f.run_case(&pg, &d.ordo);
                        let sql_rows = f.run_sql(&pg, &req.sql);
                        match (&ordo_rows, &sql_rows) {
                            (Ok(o), Ok(s)) => {
                                out["rows"] = o.clone();
                                out["sql_rows"] = s.clone();
                                let (om, sm) = (row_multiset(o), row_multiset(s));
                                if om != sm {
                                    // Same count and different contents is
                                    // the reading a reader is most likely to
                                    // get wrong, so it gets its own sentence.
                                    out["divergence"] = json!(if om.len() == sm.len() {
                                        format!(
                                            "both sides returned {} row(s) but the rows differ — the two are NOT the same query",
                                            sm.len()
                                        )
                                    } else {
                                        format!(
                                            "your SQL returned {} row(s), the decompilation {} — the two are NOT the same query",
                                            sm.len(),
                                            om.len()
                                        )
                                    });
                                }
                            }
                            (Err(e), _) => out["run_error"] = json!(e),
                            (_, Err(e)) => {
                                out["rows"] = ordo_rows.clone().unwrap_or(Value::Null);
                                out["sql_error"] = json!(e);
                            }
                        }
                    }
                }
            }
            out
        }
    }
}

pub fn schema(app: &App, fixture: &str) -> Value {
    let f = app.fixture(fixture);
    let entities: Vec<Value> = f
        .schema
        .entities
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "platform": e.platform,
                "fields": e.declared().map(|fl| {
                    let ty = match &fl.ty {
                        ordo_schema::FieldTy::Scalar(s) => s.as_str().to_string(),
                        ordo_schema::FieldTy::Ref { target, .. } => format!("-> {target}"),
                    };
                    format!("{}: {ty}{}{}", fl.name,
                        if fl.optional { "?" } else { "" },
                        if fl.sealed { " (sealed)" } else { "" })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    let ops: Vec<String> = {
        let mut v: Vec<String> = f.registry.names().cloned().collect();
        v.sort();
        v
    };
    json!({ "entities": entities, "ops": ops })
}

// ── the axum layer ───────────────────────────────────────────────────────

pub fn router(app: std::sync::Arc<App>) -> axum::Router {
    use axum::extract::State;
    use axum::response::Html;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::Arc;

    Router::new()
        .route("/", get(|| async { Html(include_str!("index.html")) }))
        .route(
            "/api/forward",
            post(|State(a): State<Arc<App>>, Json(r): Json<ForwardReq>| async move {
                Json(forward(&a, &r))
            }),
        )
        .route(
            "/api/reverse",
            post(|State(a): State<Arc<App>>, Json(r): Json<ReverseReq>| async move {
                Json(reverse(&a, &r))
            }),
        )
        .route(
            "/api/schema",
            post(|State(a): State<Arc<App>>, Json(r): Json<Value>| async move {
                Json(schema(&a, r["fixture"].as_str().unwrap_or("market")))
            }),
        )
        .route(
            "/api/write",
            post(|State(a): State<Arc<App>>, Json(r): Json<WriteReq>| async move {
                Json(write(&a, &r))
            }),
        )
        .route(
            "/api/manifest",
            post(|State(a): State<Arc<App>>, Json(r): Json<ManifestReq>| async move {
                Json(manifest(&a, &r))
            }),
        )
        .with_state(app)
}
