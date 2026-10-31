//! The door, the executor and Postgres, stood up for the refinement suites
//! (`refine_door.rs`, `refine_paging.rs`).
//!
//! The probe rows are the shared parity table's
//! (`ordo-conformance/fixtures/refine-parity.json`), the same rows the
//! device's half and the composition half read. Every refinement here goes
//! through the real door as a `$refine` and comes back from Postgres.

#![allow(dead_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_host::{FixtureExecutor, MemberAccess, DEFAULT_SPACE};
use serde_json::{json, Value};
use tower::util::ServiceExt;

pub const TABLE: &str = include_str!("../../../ordo-conformance/fixtures/refine-parity.json");

pub fn table() -> Value {
    serde_json::from_str(TABLE).expect("refine-parity.json parses")
}

/// The op for one sort of the table: `probeRows` unsorted, `probeSortN` for
/// the table's N-th distinct sort.
pub fn sorts() -> Vec<Vec<(String, bool)>> {
    let mut out: Vec<Vec<(String, bool)>> = Vec::new();
    for case in table()["cases"].as_array().expect("cases") {
        let keys: Vec<(String, bool)> = case["sort"]
            .as_array()
            .expect("sort")
            .iter()
            .map(|k| {
                (
                    k["field"].as_str().expect("field").to_string(),
                    k["direction"] == "desc",
                )
            })
            .collect();
        if !keys.is_empty() && !out.contains(&keys) {
            out.push(keys);
        }
    }
    out
}

pub fn op_for_sort(sort: &[(String, bool)]) -> String {
    if sort.is_empty() {
        return "probeRows".into();
    }
    let i = sorts()
        .iter()
        .position(|s| s == sort)
        .expect("a sort of the table");
    format!("probeSort{i}")
}

/// This suite's entities and ops, beside the market fixture (whose members
/// the door's access check reads).
fn program() -> String {
    let mut ops = String::from(
        r#"
entity probes {
  n: int?
  t: text?
  b: bool?
  m: money(2)?
  r: decimal(4)?
}

op probeRows =
  probes[id, n, t, b, m, r]

op probeNoId =
  probes[n, t]

op probeTop =
  probes[id, n]
  sort -n
  take 4

op probeTopNoId =
  probes[n]
  sort -n
  take 4

op probesByText($t: text) =
  probes(t == $t)[id, t]

entity extras {
  label: text
  d:     date?
  at:    datetime?
  units: money(0)?
  m:     money(2)?
  est:   duration(h)?
  ok:    bool?
}

op extraRows =
  extras[label, d, at, units, m, est, ok]
  sort label

entity phrases {
  p: text
}

op phraseIs($p: text) =
  phrases(p == $p)[p]

entity tokens {
  label: text
  code:  text sealed indexed refinable
}

op addToken($label: text, $code: text) =
  set tokens { label: $label, code: $code }

op tokenList =
  tokens[label, code]
  sort label

op tokenTop =
  tokens[label, code]
  sort label
  take 5

entity jottings {
  label: text
  body:  text = "" sealed refinable
}

op addJotting($label: text, $body: text) =
  set jottings { label: $label, body: $body }

op jotList =
  jottings[label, body]
  sort label

op jotTop =
  jottings[label, body]
  sort label
  take 5

entity vault {
  label:  text
  secret: text sealed indexed refinable
  std sealed
}

op addSecret($label: text, $secret: text) =
  set vault { label: $label, secret: $secret }

entity lookups {
  label: text
  code:  text sealed indexed
}

op addLookup($label: text, $code: text) =
  set lookups { label: $label, code: $code }

op lookupOf($c: text) =
  lookups(code == $c)[label, code]
  take 1

entity documents {
  title:  text
  doc:    json = {}
  secret: json? sealed
}

op addDocument($title: text, $doc: json) =
  set documents { title: $title, doc: $doc }

op saveDocument($d: id, $rev: int, $doc: json) =
  set documents { id: $d, rev: $rev, doc: $doc }

op sealDocument($d: id, $secret: json) =
  set documents { id: $d, secret: $secret }

op documentOf($d: id) =
  documents(id == $d)[title, doc, secret]

op documentsWithTheme($t: text) =
  documents(doc.theme == $t)[title]

op documentsOver($n: int) =
  documents(doc.n > $n)[title]
"#,
    );
    for (i, sort) in sorts().iter().enumerate() {
        let keys: Vec<String> = sort
            .iter()
            .map(|(f, desc)| format!("{}{f}", if *desc { "-" } else { "" }))
            .collect();
        ops.push_str(&format!(
            "\nop probeSort{i} =\n  probes[id, n, t, b, m, r]\n  sort {}\n",
            keys.join(", ")
        ));
    }
    format!("{FIXTURE}\n{ops}")
}

fn sql_text(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        other => panic!("not a cell: {other}"),
    }
}

fn seed() -> String {
    let rows: Vec<String> = table()["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| {
            let m = match r["m"].as_str() {
                Some(s) => ordo_syntax::numeric::minor_units(s, 2)
                    .expect("a money(2) value")
                    .to_string(),
                None => "NULL".into(),
            };
            let d = r["r"].as_str().unwrap_or("NULL");
            format!(
                "  (@SPACE@, {}, {}, {}, {}, {m}, {d})",
                sql_text(&r["id"]),
                sql_text(&r["n"]),
                sql_text(&r["t"]),
                sql_text(&r["b"])
            )
        })
        .collect();
    format!(
        "{SEED}\nINSERT INTO \"probes\" (\"$space\", \"id\", \"n\", \"t\", \"b\", \"m\", \"r\") VALUES\n{};\n\
         INSERT INTO \"extras\" (\"$space\", \"id\", \"label\", \"d\", \"at\", \"units\", \"m\", \"est\", \"ok\") VALUES\n\
         (@SPACE@, 'x1', 'a', DATE '2026-01-01', TIMESTAMPTZ '2026-10-05 09:00:00+00', 5, 0, 8, true),\n\
         (@SPACE@, 'x2', 'b', NULL, NULL, NULL, NULL, NULL, NULL),\n\
         (@SPACE@, 'x3', 'c', DATE '2026-06-30', TIMESTAMPTZ '2026-10-05 18:30:00+00', -3, 1250, 2, false);\n\
         INSERT INTO \"phrases\" (\"$space\", \"id\", \"p\") VALUES\n\
         (@SPACE@, 'ph1', 'say \"hi\"'),\n\
         (@SPACE@, 'ph2', 'say hi');\n",
        rows.join(",\n")
    )
}

pub fn fixture() -> Arc<Fixture> {
    let seed: &'static str = Box::leak(seed().into_boxed_str());
    Arc::new(Fixture::from_source(&program(), seed).expect("the refinement suite compiles"))
}

/// A door over its own database, serving `page_size` rows a page.
pub async fn door(db: &str, page_size: i64) -> (ApiState, Arc<Fixture>) {
    let (state, f, _) = door_with(db, page_size, ordo_exec::PoolConfig::from_env(), |k| k).await;
    (state, f)
}

/// [`door`] over a pool shaped by `cfg` (its `db` is replaced), returning
/// the pool too, so a test can hold its connections.
pub async fn door_with_pool(
    db: &str,
    page_size: i64,
    cfg: ordo_exec::PoolConfig,
) -> (ApiState, Arc<Fixture>, ordo_exec::Pool) {
    door_with(db, page_size, cfg, |k| k).await
}

/// [`door`], with the read executor's key provider wrapped by `wrap`, so a
/// suite can meter or count the releases a read takes. The write path keeps
/// the fixture's own provider.
pub async fn door_keyed(
    db: &str,
    page_size: i64,
    wrap: impl FnOnce(Arc<dyn ordo_crypto::KeyProvider>) -> Arc<dyn ordo_crypto::KeyProvider>,
) -> (ApiState, Arc<Fixture>) {
    let (state, f, _) = door_with(db, page_size, ordo_exec::PoolConfig::from_env(), wrap).await;
    (state, f)
}

/// The one builder behind [`door`], [`door_with_pool`] and [`door_keyed`].
pub async fn door_with(
    db: &str,
    page_size: i64,
    cfg: ordo_exec::PoolConfig,
    wrap: impl FnOnce(Arc<dyn ordo_crypto::KeyProvider>) -> Arc<dyn ordo_crypto::KeyProvider>,
) -> (ApiState, Arc<Fixture>, ordo_exec::Pool) {
    let f = fixture();
    let pg = Pg::named(db);
    f.reset(&pg).expect("DDL applies and the fixture seeds");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: pg.db.clone(),
        ..cfg
    })
    .await
    .expect("pool boots");
    let held = pool.clone();
    let deployment = ordo_host::deploy::Deployment::pinned(
        "refine-test",
        f.clone(),
        ordo_exec::Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: wrap(f.keys.clone()),
            space: ordo_conformance::fixture_space(),
        },
    );
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(FixtureExecutor {
            spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
            pg: pg.clone(),
            page_size,
        }),
        access: Arc::new(MemberAccess {
            fixture: f.clone(),
            pg,
        }),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"ordo-refine-door"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    (state, f, held)
}

pub async fn call(
    state: &ApiState,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call_as(state, Some("member:mem_1"), method, path, body).await
}

/// One request as `token` (`None` sends no bearer at all).
pub async fn call_as(
    state: &ApiState,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, body) = call_full(state, token, method, path, body).await;
    (status, body)
}

/// [`call_as`], with the response's headers.
pub async fn call_full(
    state: &ApiState,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-ordo-space", DEFAULT_SPACE);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = match body {
        Some(v) => req.body(Body::from(v.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `GET /op/{op}?<params>&$refine=…[&$page=…]`.
pub fn path(op: &str, params: &str, refine: Option<&Value>, page: Option<&str>) -> String {
    let mut q: Vec<String> = Vec::new();
    if !params.is_empty() {
        q.push(params.to_string());
    }
    if let Some(r) = refine {
        q.push(format!("$refine={}", encode(&r.to_string())));
    }
    if let Some(p) = page {
        q.push(format!("$page={}", encode(p)));
    }
    if q.is_empty() {
        format!("/op/{op}")
    } else {
        format!("/op/{op}?{}", q.join("&"))
    }
}

/// One refined read, every page, through the door.
pub async fn all_pages(state: &ApiState, op: &str, refine: &Value) -> Result<Vec<Value>, Value> {
    let mut rows = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..200 {
        let (status, body) = call(
            state,
            "GET",
            &path(op, "", Some(refine), page.as_deref()),
            None,
        )
        .await;
        if status != StatusCode::OK {
            return Err(json!({"status": status.as_u16(), "body": body}));
        }
        rows.extend(body["rows"].as_array().cloned().unwrap_or_default());
        match body["next"].as_str() {
            Some(next) => page = Some(next.to_string()),
            None => return Ok(rows),
        }
    }
    panic!("a listing that never ends");
}

pub fn ids(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["id"].as_str().unwrap_or("<no id>").to_string())
        .collect()
}
