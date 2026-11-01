//! Backend functions through the door, increment 1
//! (`plan-backend-functions.md` §12): the notes v2 program's `suggestTitle`
//! called with `POST /fn/suggestTitle` as a signed-in person, with a fetch
//! double standing in for the egress kernel and the model.
//!
//! The door here is `ordo_api::function_routes` over a real `ApiState`, the
//! host's [`HostFunctions`] runner, the real wasmtime function host, and the
//! code read back from `$fncode` by the hash in the program's manifest.
//! The serving binary merges the same routes into its door (`runtime::door`),
//! beside `/op/<name>`; the plan writes the path as `/v1/fn/<name>`.
//!
//! Needs a Postgres the conformance settings reach (`ORDO_PG_*`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{
    AccessChecker, AccessDecision, ApiError, ApiState, MemoryBus, MemoryReplayCache, PageKey,
};
use ordo_conformance::{Fixture, Pg};
use ordo_fn::{FetchError, FnFetch, FnFetchRequest, FnFetchResponse, HeaderValue};
use ordo_host::fn_runs;
use ordo_host::functions::{self, FnTable, HostFunctions};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const V2: &str = include_str!("../../../examples/notes-v2/notes.ordo");
const SUGGEST: &[u8] = include_bytes!("../../../examples/notes-v2/fn/suggest-title.js");

/// `person:<id>` is a signed-in person who belongs to tenant `acme` and to
/// nothing else. Any other token is refused.
struct People;

impl AccessChecker for People {
    fn check(&self, token: &str, space: &str, op: &str) -> AccessDecision {
        match token.strip_prefix("person:") {
            // `eve` may call functions and may not read files.
            Some("eve") if op == ordo_api::blob::BLOB_READ_OP => AccessDecision::Deny,
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

    async fn mac(
        &self,
        _space: &str,
        _req: ordo_fn::FnMacRequest,
    ) -> Result<ordo_fn::FnMacAnswer, FetchError> {
        panic!("this double holds no secret")
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
    door_with(db, extra, extra_code, None).await
}

/// The real egress kernel over an in-memory secret store.
struct Kernel {
    /// A TLS listener the kernel reaches as [`FAR_HOST`].
    far_end: Option<std::net::SocketAddr>,
    /// Values set through the store before the first call.
    secrets: &'static [(&'static str, &'static str)],
}

impl Kernel {
    /// `probeKey` set, the far end at `addr`.
    fn probe(addr: std::net::SocketAddr) -> Option<Kernel> {
        Some(Kernel {
            far_end: Some(addr),
            secrets: &[("probeKey", VALUE)],
        })
    }
}

/// [`door`], with `kernel` set: the real egress kernel and secret store
/// in place of the model double.
async fn door_with(
    db: &str,
    extra: &str,
    extra_code: &[(&str, &str)],
    kernel: Option<Kernel>,
) -> Door {
    door_files(db, extra, extra_code, kernel, None).await
}

/// [`door_with`], with the blob door's port and the increment 2 settings.
async fn door_files(
    db: &str,
    extra: &str,
    extra_code: &[(&str, &str)],
    kernel: Option<Kernel>,
    with_files: Option<(Arc<dyn ordo_api::blob::BlobPort>, functions::FnSettings)>,
) -> Door {
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
    let (fetch, scrub): (Arc<dyn FnFetch>, Arc<dyn functions::CallScrub>) = match kernel {
        None => (model.clone(), Arc::new(functions::NoCallScrub)),
        Some(k) => {
            let store = Arc::new(ordo_host::secrets::SecretStore::memory(
                f.keys.clone(),
                "acme",
            ));
            store.bind(&f.schema.secrets);
            for (name, value) in k.secrets {
                store
                    .set(name, value.as_bytes(), "test", 1)
                    .await
                    .expect("set");
            }
            let mut egress = ordo_egress::FnEgress::new(
                store.clone(),
                ordo_egress::TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
                ordo_egress::DenyList::categories_only().permitting_loopback_for_tests(),
            );
            if let Some(addr) = k.far_end {
                egress = egress.resolving_for_tests(FAR_HOST, addr);
            }
            (Arc::new(egress), store)
        }
    };
    let mut runner = HostFunctions::new(
        ordo_fn::HostConfig::default(),
        table,
        spaces.clone(),
        pool.clone(),
        fetch,
        scrub,
    );
    if let Some((port, settings)) = with_files {
        runner = runner.with_blobs(Some(port)).with_settings(settings);
    }
    let runner = Arc::new(runner);
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
    {
        let seen = d.model.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].url, "https://api.anthropic.com/v1/messages");
        assert!(seen[0].headers.iter().any(|(k, v)| k == "x-api-key"
            && matches!(v, HeaderValue::Secret { name, template }
                if name == "llmKey" && template == "{}")));
        assert_eq!(seen[0].egress, vec!["api.anthropic.com".to_string()]);
        assert_eq!(seen[0].secrets, vec!["llmKey".to_string()]);
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert!(sent["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("milk, eggs"));
    }

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

// ── crypto.subtle from a function ───────────────────────────────────────

const HOOK: &str = "\
function checkHook in notes {\n  call($body: text(2000), $signature: text(200)) -> json\n  code \"fn/check-hook.js\"\n}\n";

/// A webhook check: HMAC-SHA256 over the body, hex, compared with
/// `crypto.subtle.verify`.
const HOOK_CODE: &[(&str, &str)] = &[(
    "fn/check-hook.js",
    r#"
const hex = (b) => Array.from(new Uint8Array(b), (x) => x.toString(16).padStart(2, "0")).join("");
const unhex = (s) => new Uint8Array((s.match(/../g) || []).map((h) => parseInt(h, 16)));
export default {
  async call(input) {
    const enc = new TextEncoder();
    const key = await crypto.subtle.importKey(
      "raw", enc.encode("Jefe"), { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]);
    const body = enc.encode(input.body);
    const valid = await crypto.subtle.verify("HMAC", key, unhex(input.signature), body);
    const expected = hex(await crypto.subtle.sign("HMAC", key, body));
    const digest = hex(await crypto.subtle.digest("SHA-256", body));
    return { valid, expected, digest, id: crypto.randomUUID().length };
  },
};
"#,
)];

#[tokio::test(flavor = "multi_thread")]
async fn a_function_checks_a_webhook_signature_with_crypto_subtle() {
    let d = door("ordo_fn_hook", HOOK, HOOK_CODE).await;
    // RFC 4231 test case 2.
    let body = "what do ya want for nothing?";
    let good = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
    let (st, out) = d
        .post(
            "checkHook",
            "acme",
            Some("person:ana"),
            None,
            json!({ "body": body, "signature": good }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["result"]["valid"], true, "{out}");
    assert_eq!(out["result"]["expected"], good);
    assert_eq!(out["result"]["digest"].as_str().map(str::len), Some(64));
    assert_eq!(out["result"]["id"], 36);

    let bad = format!("00{}", &good[2..]);
    let (st, out) = d
        .post(
            "checkHook",
            "acme",
            Some("person:ana"),
            None,
            json!({ "body": body, "signature": bad }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["result"]["valid"], false, "{out}");
}

// ── the real egress kernel, against a TLS listener on loopback ──────────

const CA: &[u8] = include_bytes!("../../ordo-egress/tests/fixture/ca.der");
const LEAF: &[u8] = include_bytes!("../../ordo-egress/tests/fixture/leaf.der");
const LEAF_KEY: &[u8] = include_bytes!("../../ordo-egress/tests/fixture/leaf-key.der");
/// The name the fixture's leaf certificate is for.
const FAR_HOST: &str = "egress.test.invalid";
const VALUE: &str = "sk-probe-0123456789abcdef";

/// Answers `body` to every request, and sends each request it read.
async fn far_end(body: String) -> (std::net::SocketAddr, tokio::sync::mpsc::Receiver<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let cert = rustls::pki_types::CertificateDer::from(LEAF.to_vec());
    let key = rustls::pki_types::PrivateKeyDer::try_from(LEAF_KEY.to_vec()).expect("key");
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = tcp.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        while let Ok((sock, _)) = tcp.accept().await {
            let (acceptor, tx, body) = (acceptor.clone(), tx.clone(), body.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(sock).await else {
                    return;
                };
                let mut buf = vec![0u8; 16 * 1024];
                let n = tls.read(&mut buf).await.unwrap_or(0);
                let _ = tx
                    .send(String::from_utf8_lossy(&buf[..n]).into_owned())
                    .await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(resp.as_bytes()).await;
                let _ = tls.flush().await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, rx)
}

const PROBE: &str = "\
secret probeKey for egress.test.invalid\n\
function probe in notes {\n  call($port: int) -> json\n  code \"fn/probe.js\"\n  \
egress egress.test.invalid\n  secrets probeKey\n}\n";

/// Calls the far end with the key, logs what came back, and answers it.
/// With `?throw` it throws what came back instead.
const PROBE_CODE: &[(&str, &str)] = &[(
    "fn/probe.js",
    r#"export default { async call(input, ctx) {
        const r = await fetch("https://egress.test.invalid:" + input.port + "/v1/echo", {
          method: "POST",
          headers: { authorization: ctx.secret("probeKey").bearer() },
          body: "{}",
        });
        const t = await r.text();
        console.log("the far end said", t);
        return JSON.parse(t);
    } }"#,
)];

#[tokio::test(flavor = "multi_thread")]
async fn the_real_kernel_fills_the_secret_in_and_an_echoed_value_never_lands_in_fnruns() {
    // The far end echoes the key back, as a careless API does.
    let (addr, mut seen) = far_end(format!(r#"{{"seen":"Bearer {VALUE}"}}"#)).await;
    let d = door_with("ordo_fn_echo", PROBE, PROBE_CODE, Kernel::probe(addr)).await;
    let (st, body) = d
        .post(
            "probe",
            "acme",
            Some("person:ana"),
            None,
            json!({ "port": addr.port() }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // The value went out, to the bound host, in the header the code named.
    let req = seen.recv().await.expect("a request reached the far end");
    assert!(
        req.to_ascii_lowercase().contains(&format!(
            "authorization: bearer {}",
            VALUE.to_ascii_lowercase()
        )),
        "{req}"
    );
    // It never came back into the function, the answer, or the run record.
    assert!(!body.to_string().contains(VALUE), "{body}");
    assert_eq!(body["result"]["seen"], "Bearer [secret]");
    let runs = functions::runs(&d.pool, "acme").await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, "ok");
    assert!(runs[0].logs.contains("[secret]"), "{}", runs[0].logs);
    for r in &runs {
        let row = format!("{r:?}");
        assert!(!row.contains(VALUE), "the value reached $fnruns: {row}");
    }
    let raw = d
        .pool
        .rows("SELECT \"message\", \"logs\", \"hosts\" FROM \"$fnruns\"")
        .await
        .unwrap();
    assert!(!format!("{raw:?}").contains(VALUE));

    // The owner's read of the run's log shows the line, the value as
    // `[secret]`.
    let ops = run_ops(&d);
    let seq = ops
        .execute(fn_runs::RUNS, HOME, &bound(&[]))
        .await
        .unwrap()
        .rows[0]["seq"]
        .as_u64()
        .unwrap();
    let one = ops
        .execute(fn_runs::RUN, HOME, &bound(&[("seq", json!(seq))]))
        .await
        .unwrap();
    let log = one.rows[0]["logs"].to_string();
    assert!(log.contains("the far end said"), "{log}");
    assert!(log.contains("[secret]"), "{log}");
    assert!(!one.rows[0].to_string().contains(VALUE), "{}", one.rows[0]);
}

/// The deployment's own space, where the run ops answer.
const HOME: &str = "ordo_home";

fn run_ops(d: &Door) -> fn_runs::FnRunOps {
    fn_runs::FnRunOps {
        control: d.pool.clone(),
        home: HOME.into(),
    }
}

fn bound(pairs: &[(&str, Value)]) -> ordo_api::BoundParams {
    ordo_api::BoundParams(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

const CHATTY: &str =
    "function chatty in notes {\n  call($n: int) -> json\n  code \"fn/chatty.js\"\n}\n";

/// Logs at three levels, then `n` lines of 1000 characters.
const CHATTY_CODE: &[(&str, &str)] = &[(
    "fn/chatty.js",
    r#"export default { async call(input, ctx) {
        console.log("hello from", ctx.tenant);
        console.warn("careful");
        console.error("bad");
        for (let i = 0; i < input.n; i++) console.log("x".repeat(1000));
        return { ok: true };
    } }"#,
)];

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_lists_runs_newest_first_and_reads_one_runs_log() {
    let d = door("ordo_fn_runs_read", CHATTY, CHATTY_CODE).await;
    let ops = run_ops(&d);
    // No run yet: an empty list.
    let none = ops.execute(fn_runs::RUNS, HOME, &bound(&[])).await.unwrap();
    assert!(none.rows.is_empty());

    for n in [0, 0, 200] {
        let (st, body) = d
            .post(
                "chatty",
                "acme",
                Some("person:ana"),
                None,
                json!({ "n": n }),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{body}");
    }
    let (st, body) = d
        .post(
            "suggestTitle",
            "acme",
            Some("person:ana"),
            None,
            json!({ "text": "milk, eggs" }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // Newest first, every function, with no log text in the list.
    let all = ops
        .execute(fn_runs::RUNS, HOME, &bound(&[]))
        .await
        .unwrap()
        .rows;
    assert_eq!(all.len(), 4);
    assert_eq!(all[0]["function"], "suggestTitle");
    assert!(all[0]["seq"].as_u64() > all[1]["seq"].as_u64());
    assert_eq!(all[1]["tenant"], "acme");
    assert_eq!(all[1]["outcome"], "ok");
    assert_eq!(all[1]["wake"], "call");
    assert!(all[1]["principal"]
        .as_str()
        .unwrap()
        .ends_with("on behalf of ana"));
    assert!(all.iter().all(|r| r.get("logs").is_none()));
    assert!(!format!("{all:?}").contains("milk"), "{all:?}");

    // By function, and by limit.
    let chatty = ops
        .execute(
            fn_runs::RUNS,
            HOME,
            &bound(&[("function", json!("chatty")), ("limit", json!(2))]),
        )
        .await
        .unwrap()
        .rows;
    assert_eq!(chatty.len(), 2);
    assert!(chatty.iter().all(|r| r["function"] == "chatty"));

    // One run: its log lines with their levels.
    let quiet = all[3]["seq"].as_u64().unwrap();
    let one = ops
        .execute(fn_runs::RUN, HOME, &bound(&[("seq", json!(quiet))]))
        .await
        .unwrap()
        .rows;
    assert_eq!(
        one[0]["logs"],
        json!([
            {"level": "info", "text": "hello from acme"},
            {"level": "warn", "text": "careful"},
            {"level": "error", "text": "bad"},
        ])
    );
    assert_eq!(one[0]["logLines"], 3);

    // The log of a call that wrote 200 KB is cut at 64 KB and marked.
    let loud = all[1]["seq"].as_u64().unwrap();
    let one = ops
        .execute(fn_runs::RUN, HOME, &bound(&[("seq", json!(loud))]))
        .await
        .unwrap()
        .rows;
    let logs = one[0]["logs"].as_array().unwrap();
    let bytes: usize = logs.iter().map(|l| l["text"].as_str().unwrap().len()).sum();
    assert!(bytes <= 64 * 1024 + 100, "{bytes}");
    assert!(
        logs.last().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("log truncated"),
        "{:?}",
        logs.last()
    );

    // An unknown run names how long runs are kept.
    let e = ops
        .execute(fn_runs::RUN, HOME, &bound(&[("seq", json!(999_999))]))
        .await
        .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::NotFound);
    assert!(
        e.message.contains("ORDO_FN_RUN_RETENTION_DAYS"),
        "{}",
        e.message
    );

    // A tenant's space is answered nothing.
    let e = ops
        .execute(fn_runs::RUNS, "acme", &bound(&[]))
        .await
        .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_deletes_runs_older_than_the_retention_and_keeps_the_rest() {
    let d = door("ordo_fn_runs_sweep", CHATTY, CHATTY_CODE).await;
    for _ in 0..3 {
        let (st, body) = d
            .post(
                "chatty",
                "acme",
                Some("person:ana"),
                None,
                json!({ "n": 0 }),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{body}");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    const DAY: u64 = 86_400;
    // The first run 15 days old, the second 13, the last today.
    d.pool
        .rows(&format!(
            "UPDATE \"$fnruns\" SET \"started_at\" = CASE \
               WHEN \"seq\" = (SELECT min(\"seq\") FROM \"$fnruns\") THEN {} \
               WHEN \"seq\" = (SELECT max(\"seq\") FROM \"$fnruns\") THEN {now} \
               ELSE {} END",
            now - 15 * DAY,
            now - 13 * DAY
        ))
        .await
        .unwrap();

    let gone = fn_runs::sweep(&d.pool, fn_runs::DEFAULT_RETENTION_DAYS, now)
        .await
        .unwrap();
    assert_eq!(gone, 1, "only the run past 14 days goes");
    assert_eq!(functions::runs(&d.pool, "acme").await.unwrap().len(), 2);
    let left = d
        .pool
        .rows(&format!(
            "SELECT count(*) FROM \"$fnruns\" WHERE \"started_at\" < {}",
            now - 14 * DAY
        ))
        .await
        .unwrap();
    assert_eq!(left[0][0].as_deref(), Some("0"));

    // A second pass finds nothing; a shorter setting takes the next one.
    assert_eq!(fn_runs::sweep(&d.pool, 14, now).await.unwrap(), 0);
    assert_eq!(fn_runs::sweep(&d.pool, 7, now).await.unwrap(), 1);
    assert_eq!(functions::runs(&d.pool, "acme").await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_real_kernel_refuses_a_host_the_secret_is_not_bound_to() {
    let (addr, _seen) = far_end("{}".into()).await;
    // Two hosts allowed; the secret is bound to only one of them. The
    // function sends it to the other.
    let program = "\
secret probeKey for egress.test.invalid\n\
function stray in notes {\n  call() -> json\n  code \"fn/stray.js\"\n  \
egress egress.test.invalid, other.test.invalid\n  secrets probeKey\n}\n";
    let code: &[(&str, &str)] = &[(
        "fn/stray.js",
        r#"export default { async call(i, ctx) {
            await fetch("https://other.test.invalid/", { headers: { authorization: ctx.secret("probeKey").bearer() } });
            return 1;
        } }"#,
    )];
    let d = door_with("ordo_fn_stray", program, code, Kernel::probe(addr)).await;
    let (st, body) = d
        .post("stray", "acme", Some("person:ana"), None, json!({}))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "egress_refused");
    let runs = functions::runs(&d.pool, "acme").await.unwrap();
    assert_eq!(runs[0].outcome, "secret_host_mismatch");
}

// ── a webhook signature, checked with a secret the host holds ───────────

/// GitHub's documented example: this secret, this payload, this header.
const GITHUB_SECRET: &str = "It's a Secret to Everybody";
const GITHUB_PAYLOAD: &str = "Hello, World!";
const GITHUB_HEADER: &str =
    "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";

const GITHUB: &str = "\
secret githubHookKey\n\
secret unsetHookKey\n\
function checkGithubHook in notes {\n  call($key: text(40), $body: text(20000), $signature: text(200)) -> json\n  \
code \"fn/github-hook.js\"\n  secrets githubHookKey, unsetHookKey\n}\n";

/// The example in `docs/howto-functions.md`, with the key's name as input.
const GITHUB_CODE: &[(&str, &str)] = &[(
    "fn/github-hook.js",
    r#"
const unhex = (s) => new Uint8Array((s.match(/../g) || []).map((h) => parseInt(h, 16)));
export default {
  async call(input) {
    const key = await crypto.subtle.importKey(
      "secret", input.key, { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
    const [scheme, hex] = input.signature.split("=");
    if (scheme !== "sha256" || !hex) return { valid: false };
    const valid = await crypto.subtle.verify(
      "HMAC", key, unhex(hex), new TextEncoder().encode(input.body));
    return { valid };
  },
};
"#,
)];

#[tokio::test(flavor = "multi_thread")]
async fn a_function_checks_a_github_signature_with_a_secret_it_never_holds() {
    let kernel = Kernel {
        far_end: None,
        secrets: &[("githubHookKey", GITHUB_SECRET)],
    };
    let d = door_with("ordo_fn_github", GITHUB, GITHUB_CODE, Some(kernel)).await;
    let call = |body: &str, signature: &str, key: &str| json!({ "key": key, "body": body, "signature": signature });
    let (st, out) = d
        .post(
            "checkGithubHook",
            "acme",
            Some("person:ana"),
            None,
            call(GITHUB_PAYLOAD, GITHUB_HEADER, "githubHookKey"),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["result"]["valid"], true, "{out}");

    // A changed body, or a signature made with another key, is refused.
    let (_, out) = d
        .post(
            "checkGithubHook",
            "acme",
            Some("person:ana"),
            None,
            call("Hello, World?", GITHUB_HEADER, "githubHookKey"),
        )
        .await;
    assert_eq!(out["result"]["valid"], false, "{out}");

    // A declared secret with no value refuses the call and names the command.
    let (st, out) = d
        .post(
            "checkGithubHook",
            "acme",
            Some("person:ana"),
            None,
            call(GITHUB_PAYLOAD, GITHUB_HEADER, "unsetHookKey"),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{out}");
    assert_eq!(out["error"]["code"], "egress_refused");
    assert!(
        out.to_string().contains("ordo secret set unsetHookKey"),
        "{out}"
    );

    // A secret the function does not list is refused, though it is set.
    let (st, out) = d
        .post(
            "checkGithubHook",
            "acme",
            Some("person:ana"),
            None,
            call(GITHUB_PAYLOAD, GITHUB_HEADER, "llmKey"),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{out}");

    let runs = functions::runs(&d.pool, "acme").await.unwrap();
    let mut outcomes: Vec<&str> = runs.iter().map(|r| r.outcome.as_str()).collect();
    outcomes.sort();
    assert_eq!(
        outcomes,
        ["ok", "ok", "secret_not_listed", "secret_not_set"]
    );
    let raw = d
        .pool
        .rows("SELECT \"message\", \"logs\" FROM \"$fnruns\"")
        .await
        .unwrap();
    assert!(!format!("{raw:?}").contains(GITHUB_SECRET));
}

// ── files in (increment 2) ───────────────────────────────────────────────

const ANA_FILE: &str = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0";
const BEA_FILE: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0";

/// The blob door's port, as the platform's: each file has one owner and
/// the "row guard" shows it to them alone. Keeps every request it was
/// asked, so a test can see who each read ran as.
struct Shelf {
    asked: Mutex<Vec<ordo_api::blob::BlobRequest>>,
}

impl Shelf {
    fn new() -> Arc<Shelf> {
        Arc::new(Shelf {
            asked: Mutex::new(Vec::new()),
        })
    }

    fn find(&self, req: &ordo_api::blob::BlobRequest) -> Result<(&'static str, Vec<u8>), ApiError> {
        self.asked.lock().unwrap().push(req.clone());
        let files: [(&str, &str, &str, &[u8]); 2] = [
            (
                ANA_FILE,
                "ana",
                "application/pdf",
                b"%PDF-1.7 ana's invoice",
            ),
            (
                BEA_FILE,
                "bea",
                "application/pdf",
                b"%PDF-1.7 bea's payslip",
            ),
        ];
        files
            .iter()
            .find(|(h, owner, _, _)| {
                Some(*h) == req.handle_hex.as_deref() && *owner == req.principal
            })
            .map(|(_, _, mime, bytes)| (*mime, bytes.to_vec()))
            .ok_or_else(|| ApiError::new(ordo_api::ErrorCode::NotFound, "no such blob"))
    }
}

#[async_trait::async_trait]
impl ordo_api::blob::BlobPort for Shelf {
    async fn write(
        &self,
        _req: &ordo_api::blob::BlobRequest,
        _declared_len: u64,
        _mime: &str,
        _body: ordo_api::blob::BlobBody,
    ) -> Result<(), ApiError> {
        panic!("a function never writes a file")
    }

    async fn meta(
        &self,
        req: &ordo_api::blob::BlobRequest,
    ) -> Result<ordo_api::blob::BlobMeta, ApiError> {
        let (mime, bytes) = self.find(req)?;
        Ok(ordo_api::blob::BlobMeta {
            mime: mime.into(),
            size: bytes.len() as u64,
        })
    }

    async fn read(
        &self,
        req: &ordo_api::blob::BlobRequest,
        from: u64,
        len: Option<u64>,
    ) -> Result<ordo_api::blob::BlobRead, ApiError> {
        assert_eq!((from, len), (0, None), "a function reads the whole file");
        let (mime, bytes) = self.find(req)?;
        Ok(ordo_api::blob::BlobRead {
            mime: mime.into(),
            plain_len: bytes.len() as u64,
            bytes,
            from: 0,
            partial: false,
        })
    }
}

const EXTRACT: &str = "\
secret mistralKey for api.mistral.ai

function extractDocument in notes {
  call($file: blob) -> json by member
  code \"fn/extract-document.js\"
  egress api.mistral.ai
  secrets mistralKey
}
";

/// The shape of ellanor's `api/extract-document` (plan §11.2): the file
/// in, out to the OCR service as base64, JSON back.
const EXTRACT_CODE: &[(&str, &str)] = &[(
    "fn/extract-document.js",
    r#"
export default {
  async call(input, ctx) {
    const file = await ctx.blob(input.file);
    const b64 = (await file.bytes()).toBase64();
    const r = await fetch("https://api.mistral.ai/v1/ocr", {
      method: "POST",
      headers: { authorization: ctx.secret("mistralKey").bearer(), "content-type": "application/json" },
      body: JSON.stringify({
        model: "mistral-ocr-latest",
        document: { type: "document_url", document_url: "data:" + file.type + ";base64," + b64 },
      }),
    });
    const j = await r.json();
    return { type: file.type, size: file.size, text: j.content[0].text };
  },
};
"#,
)];

fn files(
    shelf: &Arc<Shelf>,
    settings: functions::FnSettings,
) -> Option<(Arc<dyn ordo_api::blob::BlobPort>, functions::FnSettings)> {
    Some((shelf.clone() as Arc<dyn ordo_api::blob::BlobPort>, settings))
}

#[tokio::test(flavor = "multi_thread")]
async fn extract_document_reads_the_callers_file_and_answers_json() {
    let shelf = Shelf::new();
    let d = door_files(
        "ordo_fn_extract",
        EXTRACT,
        EXTRACT_CODE,
        None,
        files(&shelf, Default::default()),
    )
    .await;
    let (st, body) = d
        .post(
            "extractDocument",
            "acme",
            Some("person:ana"),
            None,
            json!({ "file": ANA_FILE }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body["result"],
        json!({ "type": "application/pdf", "size": 22, "text": "Groceries for the week" })
    );

    // The read ran as ana, in her tenant, and was attributed to the function.
    {
        let asked = shelf.asked.lock().unwrap();
        assert!(!asked.is_empty());
        for r in asked.iter() {
            assert_eq!(r.principal, "ana");
            assert_eq!(r.space, "acme");
            assert_eq!(r.verb, ordo_api::blob::BlobVerb::Read);
            assert_eq!(
                r.correlation,
                format!("fn.extractDocument.blob:ana:{ANA_FILE}")
            );
        }
    }
    // The file went out as its exact bytes, base64, to the declared host.
    {
        let seen = d.model.seen.lock().unwrap();
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        let url = sent["document"]["document_url"].as_str().unwrap();
        let b64 = url.strip_prefix("data:application/pdf;base64,").unwrap();
        assert_eq!(b64, "JVBERi0xLjcgYW5hJ3MgaW52b2ljZQ==");
        assert_eq!(seen[0].url, "https://api.mistral.ai/v1/ocr");
    }
    let runs = functions::runs(&d.pool, "acme").await.unwrap();
    assert_eq!(runs.last().unwrap().outcome, "ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_the_caller_may_not_read_is_refused_as_not_found() {
    let shelf = Shelf::new();
    let d = door_files(
        "ordo_fn_blob_guard",
        EXTRACT,
        EXTRACT_CODE,
        None,
        files(&shelf, Default::default()),
    )
    .await;
    // Ana names Bea's file. The function reads as Ana, the guard hides
    // Bea's row from her, and she gets the blob door's own answer.
    let (st, body) = d
        .post(
            "extractDocument",
            "acme",
            Some("person:ana"),
            None,
            json!({ "file": BEA_FILE }),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found");
    assert!(!body.to_string().contains("payslip"), "{body}");
    assert!(d.model.seen.lock().unwrap().is_empty(), "nothing went out");
    assert!(shelf
        .asked
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.principal == "ana"));
    let runs = functions::runs(&d.pool, "acme").await.unwrap();
    assert_eq!(runs.last().unwrap().outcome, "blob_not_found");

    // Eve may call functions and may not read files at all: refused at the
    // door, before the code runs or the port is asked.
    let asked_before = shelf.asked.lock().unwrap().len();
    let (st, body) = d
        .post(
            "extractDocument",
            "acme",
            Some("person:eve"),
            None,
            json!({ "file": ANA_FILE }),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "access_denied");
    assert_eq!(shelf.asked.lock().unwrap().len(), asked_before);
    assert_eq!(functions::runs(&d.pool, "acme").await.unwrap().len(), 1);

    // A value that is not a handle is refused as input.
    let (st, body) = d
        .post(
            "extractDocument",
            "acme",
            Some("person:ana"),
            None,
            json!({ "file": "../etc/passwd" }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_param");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_body_and_file_caps_refuse_and_name_their_settings() {
    let shelf = Shelf::new();
    let d = door_files(
        "ordo_fn_caps",
        EXTRACT,
        EXTRACT_CODE,
        None,
        files(
            &shelf,
            functions::FnSettings {
                body_bytes: 2048,
                blob_bytes: 10,
                ..Default::default()
            },
        ),
    )
    .await;

    // A body over ORDO_FN_BODY_BYTES, refused before the code runs.
    let (st, body) = d
        .post(
            "suggestTitle",
            "acme",
            Some("person:ana"),
            None,
            json!({ "text": "x".repeat(4096) }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "bound_violation");
    assert!(body.to_string().contains("ORDO_FN_BODY_BYTES"), "{body}");
    assert!(d.model.seen.lock().unwrap().is_empty());

    // A file over ORDO_FN_BLOB_BYTES, refused on its size alone: the port
    // was asked for the size and never for the bytes.
    let (st, body) = d
        .post(
            "extractDocument",
            "acme",
            Some("person:ana"),
            None,
            json!({ "file": ANA_FILE }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_param");
    assert!(body.to_string().contains("ORDO_FN_BLOB_BYTES"), "{body}");
    assert_eq!(shelf.asked.lock().unwrap().len(), 1, "meta only");
    let runs = functions::runs(&d.pool, "acme").await.unwrap();
    assert_eq!(runs.last().unwrap().outcome, "blob_too_large");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_with_a_declared_length_over_the_cap_is_refused_unread() {
    let d = door_files(
        "ordo_fn_declared",
        "",
        &[],
        None,
        files(
            &Shelf::new(),
            functions::FnSettings {
                body_bytes: 1024,
                ..Default::default()
            },
        ),
    )
    .await;
    let text = json!({ "text": "y".repeat(3000) }).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/fn/suggestTitle")
        .header("x-ordo-space", "acme")
        .header("authorization", "Bearer person:ana")
        .header("content-type", "application/json")
        .header("content-length", text.len().to_string())
        .body(Body::from(text))
        .unwrap();
    let app = ordo_api::function_routes(d.state.clone(), d.runner.clone());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("3011 bytes") && msg.contains("ORDO_FN_BODY_BYTES"),
        "{msg}"
    );
}
