//! Files in (`plan-backend-functions.md` increment 2): `ctx.blob(file)`
//! reads a `blob` argument through the seam, the bytes can go out through
//! `fetch` as they are or as base64, and the host holds two lines of its
//! own: only the call's own `blob` arguments, and the per-call cap.

use std::sync::{Arc, Mutex};

use ordo_fn::{
    BlobError, FetchError, FnBlob, FnBlobs, FnCall, FnError, FnFetch, FnFetchRequest,
    FnFetchResponse, FnHost, HostConfig, Limits, NoScrub,
};
use serde_json::{json, Value};

const FILE: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210";
/// A PDF's first bytes, and one byte no UTF-8 text holds.
const PDF: &[u8] = b"%PDF-1.7\n\xff\x00end";

/// Holds files by handle, as a store would; keeps every read it was asked
/// for, and the cap each came with.
struct Files {
    files: Vec<(&'static str, &'static str, Vec<u8>)>,
    asked: Mutex<Vec<(String, u64)>>,
}

impl Files {
    fn new(files: Vec<(&'static str, &'static str, Vec<u8>)>) -> Arc<Files> {
        Arc::new(Files {
            files,
            asked: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl FnBlobs for Files {
    async fn read(&self, handle: &str, max: u64) -> Result<FnBlob, BlobError> {
        self.asked.lock().unwrap().push((handle.to_string(), max));
        let (_, mime, bytes) = self
            .files
            .iter()
            .find(|(h, _, _)| *h == handle)
            .ok_or(BlobError::NotFound)?;
        if bytes.len() as u64 > max {
            return Err(BlobError::TooLarge {
                size: bytes.len() as u64,
                left: max,
                limit: max,
            });
        }
        Ok(FnBlob {
            mime: mime.to_string(),
            bytes: bytes.clone(),
        })
    }
}

/// The OCR service: answers pages, and keeps what it was sent.
struct Ocr {
    seen: Mutex<Vec<FnFetchRequest>>,
}

#[async_trait::async_trait]
impl FnFetch for Ocr {
    async fn fetch(
        &self,
        _space: &str,
        req: FnFetchRequest,
    ) -> Result<FnFetchResponse, FetchError> {
        self.seen.lock().unwrap().push(req);
        Ok(FnFetchResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: json!({ "pages": [{ "index": 0, "markdown": "Invoice 42" }] })
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

fn call(code: &str, input: Value, blobs: Arc<dyn FnBlobs>, ocr: Arc<Ocr>) -> FnCall {
    FnCall {
        space: "acme".into(),
        function: "extractDocument".into(),
        version: "v1".into(),
        run_id: "run-1".into(),
        code: Arc::from(code.as_bytes()),
        input,
        egress: vec!["api.mistral.ai".into()],
        secrets: vec!["mistralKey".into()],
        limits: Limits::default(),
        fetch: ocr,
        scrub: Arc::new(NoScrub),
        blobs,
        blob_args: vec![FILE.into()],
    }
}

fn ocr() -> Arc<Ocr> {
    Arc::new(Ocr {
        seen: Mutex::new(Vec::new()),
    })
}

async fn host() -> FnHost {
    FnHost::new(HostConfig::default()).await.expect("engine")
}

/// The shape of ellanor's `api/extract-document` (plan §11.2).
const EXTRACT: &str = r#"
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
    if (!r.ok) throw new Error("OCR answered " + r.status);
    const j = await r.json();
    return { type: file.type, size: file.size, text: j.pages.map((p) => p.markdown).join("\n") };
  },
};
"#;

#[tokio::test(flavor = "multi_thread")]
async fn extract_document_reads_its_file_and_sends_it_out_as_base64() {
    let h = host().await;
    let files = Files::new(vec![(FILE, "application/pdf", PDF.to_vec())]);
    let o = ocr();
    let run = h
        .call(call(
            EXTRACT,
            json!({ "file": FILE }),
            files.clone(),
            o.clone(),
        ))
        .await;
    assert_eq!(
        run.outcome.unwrap(),
        json!({ "type": "application/pdf", "size": PDF.len(), "text": "Invoice 42" })
    );
    let seen = o.seen.lock().unwrap();
    let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
    let url = sent["document"]["document_url"].as_str().unwrap();
    let b64 = url.strip_prefix("data:application/pdf;base64,").unwrap();
    assert_eq!(
        b64, "JVBERi0xLjcK/wBlbmQ=",
        "the file's exact bytes, base64"
    );
    assert_eq!(run.report.blob_reads, 1);
    assert_eq!(run.report.blob_bytes, PDF.len() as u64);
    // The seam was asked with the whole default cap left.
    assert_eq!(
        *files.asked.lock().unwrap(),
        vec![(FILE.to_string(), ordo_fn::DEFAULT_BLOB_BYTES)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_goes_out_as_raw_bytes_with_its_type() {
    let code = r#"
export default {
  async call(input, ctx) {
    const file = await ctx.blob(input.file);
    await fetch("https://api.mistral.ai/v1/files", { method: "PUT", body: file });
    const back = Uint8Array.fromBase64((await file.bytes()).toBase64({ alphabet: "base64url", omitPadding: true }), { alphabet: "base64url" });
    return { same: back.length === file.size && back[9] === 0xff, text: (await new Blob(["a", new Uint8Array([98])]).text()) };
  },
};
"#;
    let h = host().await;
    let files = Files::new(vec![(FILE, "application/pdf", PDF.to_vec())]);
    let o = ocr();
    let run = h
        .call(call(code, json!({ "file": FILE }), files, o.clone()))
        .await;
    assert_eq!(run.outcome.unwrap(), json!({ "same": true, "text": "ab" }));
    let seen = o.seen.lock().unwrap();
    assert_eq!(seen[0].body, PDF, "the bytes, unchanged");
    assert!(seen[0].headers.iter().any(|(k, v)| k == "content-type"
        && matches!(v, ordo_fn::HeaderValue::Plain(t) if t == "application/pdf")));
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_calls_own_blob_arguments_are_read() {
    let code = r#"
export default {
  async call(input, ctx) {
    try { await ctx.blob(input.other); } catch (e) { return { code: e.code, name: e.name }; }
    return { code: null };
  },
};
"#;
    let h = host().await;
    // The store holds the other file, and the caller might even read it.
    // It was not passed as a `blob` argument, so it is never asked for.
    let files = Files::new(vec![(OTHER, "text/plain", b"x".to_vec())]);
    let run = h
        .call(call(
            code,
            json!({ "file": FILE, "other": OTHER }),
            files.clone(),
            ocr(),
        ))
        .await;
    assert_eq!(
        run.outcome.unwrap(),
        json!({ "code": "blob_not_argument", "name": "BlobRefused" })
    );
    assert!(files.asked.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_the_caller_may_not_read_ends_the_call_as_blob_not_found() {
    let code = r#"
export default { async call(input, ctx) { await ctx.blob(input.file); return {}; } };
"#;
    let h = host().await;
    // The seam is bound to the caller; for them this handle names nothing.
    let files = Files::new(vec![]);
    let run = h
        .call(call(code, json!({ "file": FILE }), files, ocr()))
        .await;
    let err = run.outcome.unwrap_err();
    assert_eq!(err, FnError::Blob(BlobError::NotFound));
    assert_eq!(err.code(), "blob_not_found");
    assert_eq!(run.report.blob_reads, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_per_call_cap_counts_every_file_and_names_its_setting() {
    let code = r#"
export default {
  async call(input, ctx) {
    const a = await ctx.blob(input.file);
    await ctx.blob(input.file);
    return { size: a.size };
  },
};
"#;
    let h = host().await;
    let files = Files::new(vec![(FILE, "application/pdf", vec![7u8; 600])]);
    let mut c = call(code, json!({ "file": FILE }), files.clone(), ocr());
    c.limits.blob_bytes = 1000;
    let run = h.call(c).await;
    let err = run.outcome.unwrap_err();
    assert_eq!(err.code(), "blob_too_large");
    assert!(
        err.to_string().contains("ORDO_FN_BLOB_BYTES"),
        "the refusal names the setting: {err}"
    );
    // The second read was asked with what was left, so the seam refused
    // it before any byte moved.
    let asked = files.asked.lock().unwrap();
    assert_eq!(asked[1].1, 400);
    assert_eq!(run.report.blob_bytes, 600);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_seam_that_ignores_the_cap_is_still_held_to_it() {
    struct Careless;
    #[async_trait::async_trait]
    impl FnBlobs for Careless {
        async fn read(&self, _h: &str, _max: u64) -> Result<FnBlob, BlobError> {
            Ok(FnBlob {
                mime: "application/octet-stream".into(),
                bytes: vec![0u8; 2048],
            })
        }
    }
    let code =
        r#"export default { async call(input, ctx) { await ctx.blob(input.file); return {}; } };"#;
    let h = host().await;
    let mut c = call(code, json!({ "file": FILE }), Arc::new(Careless), ocr());
    c.limits.blob_bytes = 1024;
    let run = h.call(c).await;
    assert_eq!(run.outcome.unwrap_err().code(), "blob_too_large");
    assert_eq!(run.report.blob_bytes, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn base64_round_trips_and_refuses_what_is_not_base64() {
    let code = r#"
export default {
  async call() {
    const u = new Uint8Array([0, 1, 2, 250, 251, 252, 253]);
    const s = u.toBase64();
    const back = Array.from(Uint8Array.fromBase64(s));
    let bad = null;
    try { Uint8Array.fromBase64("ab$c"); } catch (e) { bad = e.name; }
    return { s, back, url: new Uint8Array([251, 255]).toBase64({ alphabet: "base64url" }), bad };
  },
};
"#;
    let h = host().await;
    let run = h
        .call(call(code, json!({}), Files::new(vec![]), ocr()))
        .await;
    assert_eq!(
        run.outcome.unwrap(),
        json!({ "s": "AAEC+vv8/Q==", "back": [0, 1, 2, 250, 251, 252, 253], "url": "-_8=", "bad": "SyntaxError" })
    );
}
