//! Backend functions through the door, increment 1
//! (`plan-backend-functions.md` §12): the notes v2 program's `suggestTitle`
//! called with `POST /fn/suggestTitle` as a signed-in person, with a fetch
//! double standing in for the egress kernel and the model.
//!
//! The door here is `ordo_api::function_routes` over a real `ApiState`, the
//! host's [`HostFunctions`] runner, the real wasmtime function host, and the
//! code read back from `$fncode` by the hash in the program's manifest.
//! `/v1` is the prefix the serving binary nests these routes under.
//!
//! Needs a Postgres the conformance settings reach (`ORDO_PG_*`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{AccessChecker, AccessDecision, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_fn::{FetchError, FnFetch, FnFetchRequest, FnFetchResponse, HeaderValue, NoScrub};
use ordo_host::functions::{self, FnTable, HostFunctions};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const V2: &str = include_str!("../../../examples/notes-v2/notes.ordo");
const SUGGEST: &[u8] = include_bytes!("../../../examples/notes-v2/fn/suggest-title.js");

/// `person:<id>` is a signed-in person who belongs to tenant `acme` and to
/// nothing else. Any other token is refused.
struct People;

impl AccessChecker for People {
    fn check(&self, token: &str, space: &str, _op: &str) -> AccessDecision {
        match token.strip_prefix("person:") {
            Some(id) if space.starts_with("acme") => AccessDecision::allow(id),
            _ => AccessDecision::Deny,
        }
    }
}

/// The model, answering a title, and what it was sent.
struct Model {
    seen: Mutex<Vec<FnFetchRequest>>,
}

#[async_trait::async_trait]
impl FnFetch for Model {
    async fn fetch(
        &self,
        _space: &str,
        req: FnFetchRequest,
    ) -> Result<FnFetchResponse, FetchError> {
        self.seen.lock().unwrap().push(req);
        Ok(FnFetchResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: json!({
                "content": [{ "type": "text", "text": "Groceries for the week" }]
            })
            .to_string()
            .into_bytes(),
        })
    }
}

struct Door {
    state: ApiState,
    runner: Arc<HostFunctions>,
    model: Arc<Model>,
    pool: ordo_exec::Pool,
    spaces: Arc<ordo_host::spaces::Spaces>,
}

/// The notes v2 program, plus `extra` functions (with their code), behind a
/// door. Each test gets its own database.
async fn door(db: &str, extra: &str, extra_code: &[(&str, &str)]) -> Door {
    let source = format!("{V2}\n{extra}");
    let f = Arc::new(Fixture::from_source(&source, "").expect("the program compiles"));
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {db}"));
    admin.exec(&format!("CREATE DATABASE {db}")).expect("db");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");

    // What `ordo apply` does: hash each code file into the manifest, store
    // the code by hash, and take the version over both.
    let mut doc = ordo_plan::artifact::program_artifact(&f.schema, &f.registry).unwrap();
    let mut hashes = BTreeMap::new();
    let mut files: Vec<(&str, &[u8])> = vec![("fn/suggest-title.js", SUGGEST)];
    for (path, code) in extra_code {
        files.push((path, code.as_bytes()));
    }
    for (path, code) in files {
        let h = functions::store_code(&pool, code).await.expect("stored");
        hashes.insert(path.to_string(), h);
    }
    ordo_plan::artifact::bind_function_code(&mut doc, &hashes).unwrap();
    let ddl = ordo_schema::ddl_for(&f.schema).unwrap();
    let version = ordo_plan::artifact::program_version(&doc, &ddl);
    let table = FnTable::load(&pool, &f.schema, &doc, &version)
        .await
        .expect("the functions load");
    assert!(table.by_name.contains_key("suggestTitle"));

    let deployment = ordo_host::deploy::Deployment::pinned(
        "fn-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let model = Arc::new(Model {
        seen: Mutex::new(Vec::new()),
    });
    let runner = Arc::new(HostFunctions::new(
        ordo_fn::HostConfig::default(),
        table,
        spaces.clone(),
        pool.clone(),
        model.clone(),
        Arc::new(NoScrub),
    ));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(ordo_host::FixtureExecutor {
            spaces: spaces.clone(),
            pg,
            page_size: 50,
        }),
        access: Arc::new(People),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"ordo-fn-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Door {
        state,
        runner,
        model,
        pool,
        spaces,
    }
}

impl Door {
    async fn post(
        &self,
        name: &str,
        space: &str,
        token: Option<&str>,
        key: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method("POST")
            .uri(format!("/fn/{name}"))
            .header("x-ordo-space", space)
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        if let Some(k) = key {
            req = req.header("idempotency-key", k);
        }
        let req = req.body(Body::from(body.to_string())).unwrap();
        let app = ordo_api::function_routes(self.state.clone(), self.runner.clone());
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn suggest_title_runs_as_the_signed_in_person_and_answers_json() {
    let d = door("ordo_fn_suggest", "", &[]).await;
    let (st, body) = d
        .post(
            "suggestTitle",
            "acme",
            Some("person:ana"),
            None,
            json!({ "text": "milk, eggs, bread, coffee" }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "result": { "title": "Groceries for the week" } })
    );

    // What went out: one request to the declared host, the key as a handle
    // the kernel fills in, and the declaration's lists for it to check again.
    let seen = d.model.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url, "https://api.anthropic.com/v1/messages");
    assert!(seen[0].headers.contains(&(
        "x-api-key".into(),
        HeaderValue::Secret {
            name: "llmKey".into(),
            template: "{}".into()
        }
    )));
    assert_eq!(seen[0].egress, vec!["api.anthropic.com".to_string()]);
    assert_eq!(seen[0].secrets, vec!["llmKey".to_string()]);
    let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert!(sent["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("milk, eggs"));

    // One `$fnruns` row: who, as what, the outcome, the host. No input.
    let runs = functions::runs(&d.pool, "acme").await.unwrap();
    assert_eq!(runs.len(), 1);
    let r = &runs[0];
    assert_eq!(r.function, "suggestTitle");
    assert_eq!(r.outcome, "ok");
    assert!(
        r.principal.starts_with("function:suggestTitle@"),
        "{}",
        r.principal
    );
    assert!(r.principal.ends_with("on behalf of ana"), "{}", r.principal);
    assert_eq!(r.fetches, 1);
    assert_eq!(r.hosts, "api.anthropic.com");
    assert!(
        !r.logs.contains("milk"),
        "the input is never logged: {}",
        r.logs
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_door_refuses_a_stranger_and_answers_an_unknown_function_the_same_way() {
    let d = door("ordo_fn_access", "", &[]).await;
    let body = json!({ "text": "x" });
    let (no_token, a) = d
        .post("suggestTitle", "acme", None, None, body.clone())
        .await;
    let (other_tenant, b) = d
        .post(
            "suggestTitle",
            "zeta",
            Some("person:ana"),
            None,
            body.clone(),
        )
        .await;
    let (unknown, c) = d
        .post(
            "noSuchFunction",
            "acme",
            Some("person:ana"),
            None,
            body.clone(),
        )
        .await;
    let (_, e) = d
        .post(
            "suggestTitle",
            "zeta",
            Some("person:ana"),
            None,
            body.clone(),
        )
        .await;
    assert_eq!(no_token, StatusCode::FORBIDDEN, "{a}");
    assert_eq!(other_tenant, StatusCode::FORBIDDEN, "{b}");
    assert_eq!(b["error"]["code"], "access_denied");
    assert_eq!(unknown, StatusCode::FORBIDDEN, "{c}");
    assert_eq!(
        c["error"]["code"], e["error"]["code"],
        "an unknown name looks like a forbidden one"
    );
    assert!(d.model.seen.lock().unwrap().is_empty(), "nothing ran");
    assert!(functions::runs(&d.pool, "zeta").await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parameter_of_the_wrong_type_is_refused_before_anything_runs() {
    let d = door("ordo_fn_params", "", &[]).await;
    let (st, body) = d
        .post(
            "suggestTitle",
            "acme",
            Some("person:ana"),
            None,
            json!({ "text": 5 }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(d.model.seen.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_tenant_refuses_with_its_pause_answer() {
    let d = door("ordo_fn_pause", "", &[]).await;
    d.spaces.hold_paused(ordo_host::pause::PauseRecord {
        space: "acme-paused".into(),
        at: 3,
        by: ordo_host::tenants::PAUSED_BY_DEPLOYMENT.into(),
        note: "maintenance".into(),
    });
    let (st, body) = d
        .post(
            "suggestTitle",
            "acme-paused",
            Some("person:ana"),
            None,
            json!({ "text": "x" }),
        )
        .await;
    assert_eq!(st, StatusCode::LOCKED, "{body}");
    assert_eq!(body["error"]["code"], "space_paused");
    assert!(d.model.seen.lock().unwrap().is_empty(), "no code ran");
    // Another tenant is not held.
    let (st, _) = d
        .post(
            "suggestTitle",
            "acme",
            Some("person:ana"),
            None,
            json!({ "text": "x" }),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_idempotency_key_replays_the_answer_without_running_again() {
    let d = door("ordo_fn_idem", "", &[]).await;
    let body = json!({ "text": "milk" });
    let first = d
        .post(
            "suggestTitle",
            "acme",
            Some("person:ana"),
            Some("k-1"),
            body.clone(),
        )
        .await;
    let again = d
        .post(
            "suggestTitle",
            "acme",
            Some("person:ana"),
            Some("k-1"),
            body.clone(),
        )
        .await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(first, again);
    assert_eq!(d.model.seen.lock().unwrap().len(), 1, "it ran once");
    let other = d
        .post(
            "suggestTitle",
            "acme",
            Some("person:ana"),
            Some("k-1"),
            json!({ "text": "eggs" }),
        )
        .await;
    assert_eq!(
        other.1["error"]["code"], "idempotency_mismatch",
        "{}",
        other.1
    );
}

const LAB: &str = "\
function spin in notes {\n  call() -> json\n  code \"fn/spin.js\"\n  limit cpu 200ms\n}\n\
function grow in notes {\n  call() -> json\n  code \"fn/grow.js\"\n  limit memory 16 mb\n}\n\
function boom in notes {\n  call() -> json\n  code \"fn/boom.js\"\n}\n\
function remember in notes {\n  call() -> json\n  code \"fn/remember.js\"\n}\n";

const LAB_CODE: &[(&str, &str)] = &[
    ("fn/spin.js", "export default { call() { for (;;) {} } }"),
    (
        "fn/grow.js",
        "export default { call() { const a = []; for (;;) a.push('y'.repeat(1 << 16) + a.length); } }",
    ),
    ("fn/boom.js", "export default { call() { throw new Error('no title today'); } }"),
    (
        "fn/remember.js",
        "globalThis.n = (globalThis.n || 0) + 1; export default { call() { return { n: globalThis.n }; } }",
    ),
];

#[tokio::test(flavor = "multi_thread")]
async fn limits_and_failures_answer_their_typed_classes() {
    let d = door("ordo_fn_limits", LAB, LAB_CODE).await;
    let call = |name: &'static str| d.post(name, "acme", Some("person:ana"), None, json!({}));

    let (st, body) = call("spin").await;
    assert_eq!(st, StatusCode::GATEWAY_TIMEOUT, "{body}");
    assert_eq!(body["error"]["code"], "function_timeout");

    let (st, body) = call("grow").await;
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"]["code"], "function_oom");

    let (st, body) = call("boom").await;
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"]["code"], "function_error");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no title today"));

    // A fresh instance per call: module state starts over every time.
    for _ in 0..3 {
        let (st, body) = call("remember").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["result"], json!({ "n": 1 }));
    }

    let outcomes: Vec<String> = functions::runs(&d.pool, "acme")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.outcome)
        .collect();
    assert_eq!(
        outcomes,
        [
            "function_timeout",
            "function_oom",
            "function_error",
            "ok",
            "ok",
            "ok"
        ]
    );
}
