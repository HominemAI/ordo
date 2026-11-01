//! The engine cache end to end, on the real engine: a cold start compiles
//! and writes, the next start loads, a corrupt file compiles again, and a
//! call runs the same either way. Prints the start times.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ordo_fn::{
    CacheOutcome, CacheWrite, FetchError, FnCall, FnFetch, FnFetchRequest, FnFetchResponse, FnHost,
    HostConfig, Limits, NoScrub,
};
use serde_json::json;

struct NoFetch;

#[async_trait::async_trait]
impl FnFetch for NoFetch {
    async fn fetch(&self, _: &str, _: FnFetchRequest) -> Result<FnFetchResponse, FetchError> {
        panic!("no outbound call here")
    }

    async fn mac(
        &self,
        _space: &str,
        _req: ordo_fn::FnMacRequest,
    ) -> Result<ordo_fn::FnMacAnswer, FetchError> {
        panic!("this double holds no secret")
    }
}

async fn start(dir: Option<PathBuf>) -> FnHost {
    FnHost::unshared(HostConfig {
        engine_cache: dir,
        ..HostConfig::default()
    })
    .await
    .expect("engine")
}

/// Runs one call and checks its answer; returns the instantiate time.
async fn answers(h: &FnHost) -> Duration {
    let code = r#"export default { async call(i) {
        const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(i.s));
        return new Uint8Array(d)[0];
    } }"#;
    let run = h
        .call(FnCall {
            space: "acme".into(),
            function: "f".into(),
            version: "v1".into(),
            run_id: "run-1".into(),
            code: Arc::from(code.as_bytes()),
            input: json!({ "s": "abc" }),
            egress: vec![],
            secrets: vec![],
            limits: Limits::default(),
            fetch: Arc::new(NoFetch),
            scrub: Arc::new(NoScrub),
            blobs: Arc::new(ordo_fn::NoBlobs),
            blob_args: Vec::new(),
        })
        .await;
    // SHA-256("abc") starts with 0xba.
    assert_eq!(run.outcome.expect("the call"), json!(0xba));
    run.report.instantiate
}

/// The median instantiate time over 21 calls.
async fn instantiate_p50(h: &FnHost) -> Duration {
    let mut t = Vec::new();
    for _ in 0..21 {
        t.push(answers(h).await);
    }
    t.sort();
    t[10]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_loads_the_engine_from_disk() {
    let dir = std::env::temp_dir().join(format!("ordo-fn-engine-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let off = start(None).await;
    assert_eq!(off.engine_start().cache, CacheOutcome::Off);
    answers(&off).await;

    let cold = start(Some(dir.clone())).await;
    let s = cold.engine_start().clone();
    assert_eq!(s.cache, CacheOutcome::Miss);
    let CacheWrite::Written(file) = &s.write else {
        panic!("not written: {:?}", s.write);
    };
    let cold_inst = instantiate_p50(&cold).await;

    let warm = start(Some(dir.clone())).await;
    let w = warm.engine_start().clone();
    assert_eq!(w.cache, CacheOutcome::Hit(file.clone()));
    assert_eq!(w.write, CacheWrite::Skipped);
    let warm_inst = instantiate_p50(&warm).await;
    // A file is never changed in place while mapped; drop the hosts first.
    drop((cold, warm));

    // Corrupt it: the next start compiles, answers, and writes a good one.
    let mut bytes = std::fs::read(file).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    std::fs::write(file, &bytes).unwrap();
    let again = start(Some(dir.clone())).await;
    assert!(
        matches!(again.engine_start().cache, CacheOutcome::Rejected(_)),
        "{:?}",
        again.engine_start().cache
    );
    assert!(matches!(again.engine_start().write, CacheWrite::Written(_)));
    answers(&again).await;
    let healed = start(Some(dir.clone())).await;
    assert!(matches!(healed.engine_start().cache, CacheOutcome::Hit(_)));

    eprintln!(
        "engine start: no cache {:?}; cold (compile and write) {:?}; warm (load) {:?}; healed {:?}",
        off.engine_start().took,
        s.took,
        w.took,
        healed.engine_start().took
    );
    eprintln!("instantiate p50: compiled engine {cold_inst:?}; loaded engine {warm_inst:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
