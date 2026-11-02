//! The function host end to end: real engine, real wasmtime, a fetch double.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ordo_fn::{
    FetchError, FnCall, FnError, FnFetch, FnFetchRequest, FnFetchResponse, FnHost, HeaderValue,
    HostConfig, Limit, Limits, Scrub,
};
use serde_json::{json, Value};

/// Answers every request with `answer`, and keeps what it was sent.
struct Double {
    answer: Value,
    seen: Mutex<Vec<FnFetchRequest>>,
}

impl Double {
    fn new(answer: Value) -> Arc<Double> {
        Arc::new(Double {
            answer,
            seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl FnFetch for Double {
    async fn fetch(
        &self,
        _space: &str,
        req: FnFetchRequest,
    ) -> Result<FnFetchResponse, FetchError> {
        self.seen.lock().unwrap().push(req);
        Ok(FnFetchResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: self.answer.to_string().into_bytes(),
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

/// Knows one live secret value.
struct OneSecret;

impl Scrub for OneSecret {
    fn scrub(&self, _space: &str, text: &str) -> String {
        text.replace("sk-live-value", "[secret]")
    }
}

fn call(code: &str, input: Value, fetch: Arc<dyn FnFetch>) -> FnCall {
    FnCall {
        space: "acme".into(),
        function: "f".into(),
        version: "v1".into(),
        run_id: "run-1".into(),
        code: Arc::from(code.as_bytes()),
        input,
        egress: vec!["api.example.com".into()],
        secrets: vec!["llmKey".into()],
        limits: Limits::default(),
        fetch,
        scrub: Arc::new(OneSecret),
        blobs: Arc::new(ordo_fn::NoBlobs),
        blob_args: Vec::new(),
    }
}

async fn host() -> FnHost {
    FnHost::new(HostConfig::default()).await.expect("engine")
}

const SUGGEST: &str = r#"
export default {
  async call(input, ctx) {
    const r = await fetch("https://api.example.com/v1/chat", {
      method: "POST",
      headers: { authorization: ctx.secret("llmKey").bearer(), "content-type": "application/json" },
      body: JSON.stringify({ prompt: "Title for: " + input.text }),
    });
    if (!r.ok) throw new Error("model answered " + r.status);
    const j = await r.json();
    console.log("suggested", j.title, "for", ctx.tenant);
    return { title: j.title };
  },
};
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_function_fetches_with_a_secret_handle_and_returns_json() {
    let h = host().await;
    let d = Double::new(json!({ "title": "Groceries" }));
    let run = h
        .call(call(SUGGEST, json!({ "text": "milk, eggs" }), d.clone()))
        .await;
    assert_eq!(run.outcome.unwrap(), json!({ "title": "Groceries" }));
    let seen = d.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].url, "https://api.example.com/v1/chat");
    assert_eq!(seen[0].egress, vec!["api.example.com".to_string()]);
    assert_eq!(seen[0].secrets, vec!["llmKey".to_string()]);
    assert!(seen[0].headers.iter().any(|(k, v)| k == "authorization"
        && matches!(v, HeaderValue::Secret { name, template }
            if name == "llmKey" && template == "Bearer {}")));
    let body: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(body["prompt"], "Title for: milk, eggs");
    assert_eq!(run.report.fetches, 1);
    assert_eq!(run.report.hosts, vec!["api.example.com".to_string()]);
    assert_eq!(run.report.logs[0].text, "suggested Groceries for acme");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_off_the_egress_list_is_refused_before_the_kernel() {
    let h = host().await;
    let d = Double::new(json!({}));
    let code =
        r#"export default { async call() { await fetch("https://elsewhere.com/"); return 1; } }"#;
    let run = h.call(call(code, json!(null), d.clone())).await;
    match run.outcome {
        Err(FnError::Fetch { code, .. }) => assert_eq!(code, "host_not_allowed"),
        other => panic!("{other:?}"),
    }
    assert!(d.seen.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_undeclared_secret_is_refused_and_a_caught_refusal_is_the_functions_choice() {
    let h = host().await;
    let d = Double::new(json!({}));
    let code = r#"export default { async call(i, ctx) {
        try { await fetch("https://api.example.com/", { headers: { x: ctx.secret("other") } }); }
        catch (e) { return { caught: e.code }; }
    } }"#;
    let run = h.call(call(code, json!(null), d.clone())).await;
    assert_eq!(
        run.outcome.unwrap(),
        json!({ "caught": "secret_not_listed" })
    );
    assert!(d.seen.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thrown_error_is_function_error_scrubbed_and_cannot_claim_a_host_code() {
    let h = host().await;
    let code = r#"export default { call() { const e = new Error("boom sk-live-value"); e.code = "host_not_allowed"; throw e; } }"#;
    let run = h
        .call(call(code, json!(null), Double::new(json!({}))))
        .await;
    assert_eq!(run.class(), "function_error");
    match run.outcome {
        Err(FnError::Threw { message, .. }) => assert_eq!(message, "boom [secret]"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn logs_are_scrubbed_and_capped() {
    let h = host().await;
    let code = r#"export default { call() {
        console.log("key sk-live-value");
        for (let i = 0; i < 200; i++) console.log("x".repeat(100));
        return 1;
    } }"#;
    let mut c = call(code, json!(null), Double::new(json!({})));
    c.limits.log_bytes = 1000;
    let run = h.call(c).await;
    assert_eq!(run.outcome.unwrap(), json!(1));
    assert_eq!(run.report.logs[0].text, "key [secret]");
    assert!(run.report.logs_truncated);
    assert!(run.report.logs.len() < 15);
}

#[tokio::test(flavor = "multi_thread")]
async fn no_state_survives_from_one_call_to_the_next() {
    let h = host().await;
    let code = r#"
      globalThis.count = (globalThis.count || 0) + 1;
      let mine = 0;
      export default { call() { mine += 1; Object.prototype.leak = "x"; return { count: globalThis.count, mine }; } }
    "#;
    for _ in 0..3 {
        let run = h
            .call(call(code, json!(null), Double::new(json!({}))))
            .await;
        assert_eq!(run.outcome.unwrap(), json!({ "count": 1, "mine": 1 }));
    }
    let probe = r#"export default { call() { return { leak: ({}).leak ?? null, count: globalThis.count ?? null }; } }"#;
    let run = h
        .call(call(probe, json!(null), Double::new(json!({}))))
        .await;
    assert_eq!(run.outcome.unwrap(), json!({ "leak": null, "count": null }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_loop_stops_at_the_cpu_limit() {
    let h = host().await;
    let code = r#"export default { call() { for (;;) {} } }"#;
    let mut c = call(code, json!(null), Double::new(json!({})));
    c.limits.cpu = Duration::from_millis(200);
    let run = h.call(c).await;
    match run.outcome {
        Err(FnError::Timeout {
            limit: Limit::Cpu, ..
        }) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(run.class(), "function_timeout");
}

/// A fetch that never answers: only the wall limit can end the call.
struct Hang;

#[async_trait::async_trait]
impl FnFetch for Hang {
    async fn fetch(
        &self,
        _space: &str,
        _req: FnFetchRequest,
    ) -> Result<FnFetchResponse, FetchError> {
        futures::future::pending().await
    }

    async fn mac(
        &self,
        _space: &str,
        _req: ordo_fn::FnMacRequest,
    ) -> Result<ordo_fn::FnMacAnswer, FetchError> {
        panic!("this double holds no secret")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hung_fetch_stops_at_the_wall_limit() {
    let h = host().await;
    let code =
        r#"export default { async call() { await fetch("https://api.example.com/"); return 1; } }"#;
    let mut c = call(code, json!(null), Arc::new(Hang));
    c.limits.wall = Duration::from_millis(300);
    let t = Instant::now();
    let run = h.call(c).await;
    match run.outcome {
        Err(FnError::Timeout {
            limit: Limit::Wall, ..
        }) => {}
        other => panic!("{other:?}"),
    }
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_loop_stops_at_the_wall_limit_when_cpu_allows_more() {
    let h = host().await;
    let code = r#"export default { call() { for (;;) {} } }"#;
    let mut c = call(code, json!(null), Double::new(json!({})));
    c.limits.cpu = Duration::from_secs(60);
    c.limits.wall = Duration::from_millis(300);
    let run = h.call(c).await;
    match run.outcome {
        Err(FnError::Timeout {
            limit: Limit::Wall, ..
        }) => {}
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_allocation_past_the_memory_limit_is_function_oom() {
    let h = host().await;
    let code = r#"export default { call() {
        const a = [];
        for (;;) a.push("y".repeat(1 << 16) + a.length);
    } }"#;
    let mut c = call(code, json!(null), Double::new(json!({})));
    c.limits.memory_bytes = 48 * 1024 * 1024;
    c.limits.cpu = Duration::from_secs(60);
    let run = h.call(c).await;
    match run.outcome {
        Err(FnError::OutOfMemory { limit_bytes }) => assert_eq!(limit_bytes, 48 * 1024 * 1024),
        other => panic!("{other:?}"),
    }
    assert_eq!(run.class(), "function_oom");
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_over_the_tenant_cap_are_busy() {
    let h = FnHost::new(HostConfig {
        per_space: 1,
        ..HostConfig::default()
    })
    .await
    .unwrap();
    let code = r#"export default { async call() { await fetch("https://api.example.com/"); } }"#;
    let mut slow = call(code, json!(null), Arc::new(Hang));
    slow.limits.wall = Duration::from_millis(800);
    let h = Arc::new(h);
    let h2 = h.clone();
    let first = tokio::spawn(async move { h2.call(slow).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let run = h.call(call(code, json!(null), Arc::new(Hang))).await;
    assert_eq!(run.class(), "function_busy");
    first.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn instantiate_latency_is_measured() {
    // Run alone (`--test-threads 1`) to read the first line: it is the
    // engine compile and snapshot only when this test builds it.
    let t = Instant::now();
    let h = host().await;
    eprintln!("engine ready in {:?}", t.elapsed());
    let code = r#"export default { call(i) { return i; } }"#;
    let mut inst = Vec::new();
    let mut total = Vec::new();
    let mut peak = 0;
    for n in 0..20 {
        let run = h
            .call(call(code, json!({ "n": n }), Double::new(json!({}))))
            .await;
        assert_eq!(run.outcome.unwrap(), json!({ "n": n }));
        inst.push(run.report.instantiate);
        total.push(run.report.wall);
        peak = peak.max(run.report.peak_memory);
    }
    inst.sort();
    total.sort();
    eprintln!(
        "instantiate p50 {:?} max {:?}; whole call p50 {:?} max {:?}; peak memory {} bytes",
        inst[10], inst[19], total[10], total[19], peak
    );
    // A fresh instance from the snapshot is cheap; a regression to booting
    // the engine per call would cost milliseconds.
    assert!(inst[10] < Duration::from_millis(5), "{:?}", inst[10]);
}

#[tokio::test(flavor = "multi_thread")]
async fn fuel_rate_is_measured() {
    let h = host().await;
    let code = r#"export default { call() { let s = 0; for (let i = 0; i < 300000; i++) s += i; return s; } }"#;
    let mut c = call(code, json!(null), Double::new(json!({})));
    c.limits.cpu = Duration::from_secs(60);
    let t = Instant::now();
    let run = h.call(c).await;
    let el = t.elapsed();
    assert!(run.outcome.is_ok());
    eprintln!(
        "fuel {} in {:?}: {} fuel per ms",
        run.report.fuel,
        el,
        run.report.fuel / (el.as_millis().max(1) as u64)
    );
}
