//! The blob door as a route: the door check, the caps, the ranges, and
//! the two headers that stop an uploaded file becoming stored XSS.
//!
//! A fake [`BlobPort`] stands in for the platform, so what is exercised
//! here is exactly the half `ordo-api` owns — the routing, the access
//! check, the `Content-Length` gate, the `Range` conversion and the
//! response headers. The half the port owns (the row guard, the sealer,
//! the store) is `ordo-host`'s and `ordo-blob`'s, and is tested there
//! against a real store and real custody rather than mocked here.

use std::sync::Arc;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::blob::{BlobBody, BlobLimits, BlobPort, BlobRead, BlobRequest, BlobState};
use ordo_api::spec::{AccessChecker, AccessDecision, TokenGate};
use ordo_api::{router_with_blobs, ApiError, ApiState, ErrorCode, MapRegistry, RegistryHandle};
use tower::ServiceExt;

const SPACE: &str = "spc_01";

struct Access;

#[async_trait::async_trait]
impl AccessChecker for Access {
    fn check(&self, token: &str, space: &str, op: &str) -> AccessDecision {
        if token != "tok-ok" || space != SPACE {
            return AccessDecision::Deny;
        }
        // `documents` is readable and writable; `secrets` is neither — so
        // the test can prove the check is made against the row's own std
        // op rather than being waved through for every entity.
        if op.starts_with("std.secrets.") {
            return AccessDecision::Deny;
        }
        AccessDecision::Allow {
            principal: "usr_ok".into(),
            agent: None,
        }
    }
    async fn token_gate(&self, _token: &str) -> TokenGate {
        TokenGate::Pass
    }
}

#[derive(Default)]
struct FakePort {
    /// Every write the door let through, as (correlation, declared, mime,
    /// bytes actually delivered).
    writes: Mutex<Vec<(String, u64, String, usize)>>,
    reads: Mutex<Vec<(u64, Option<u64>)>>,
    /// The type the stored file was uploaded as; empty means `application/pdf`.
    mime: Mutex<String>,
}

/// [`Access`], with the anonymous door open on [`SPACE`].
struct OpenAccess;

#[async_trait::async_trait]
impl AccessChecker for OpenAccess {
    fn check(&self, token: &str, space: &str, op: &str) -> AccessDecision {
        Access.check(token, space, op)
    }
    async fn token_gate(&self, _token: &str) -> TokenGate {
        TokenGate::Pass
    }
    fn anonymous_space(&self) -> Option<String> {
        Some(SPACE.into())
    }
}

#[async_trait::async_trait]
impl BlobPort for FakePort {
    async fn write(
        &self,
        req: &BlobRequest,
        declared_len: u64,
        mime: &str,
        mut body: BlobBody,
    ) -> Result<(), ApiError> {
        use futures::StreamExt;
        let mut got = 0usize;
        while let Some(chunk) = body.next().await {
            got += chunk
                .map_err(|e| ApiError::new(ErrorCode::Internal, e))?
                .len();
        }
        self.writes.lock().unwrap().push((
            req.correlation.clone(),
            declared_len,
            mime.to_string(),
            got,
        ));
        Ok(())
    }

    async fn meta(&self, req: &BlobRequest) -> Result<ordo_api::BlobMeta, ApiError> {
        if req.handle_hex.as_deref() == Some(&"0".repeat(32)) {
            return Err(ApiError::new(ErrorCode::NotFound, "no such blob"));
        }
        Ok(ordo_api::BlobMeta {
            mime: "application/pdf".into(),
            size: 100,
        })
    }

    async fn read(
        &self,
        req: &BlobRequest,
        from: u64,
        len: Option<u64>,
    ) -> Result<BlobRead, ApiError> {
        self.reads.lock().unwrap().push((from, len));
        if req.handle_hex.as_deref() == Some(&"0".repeat(32)) {
            return Err(ApiError::new(ErrorCode::NotFound, "no such blob"));
        }
        let whole: Vec<u8> = (0..100u8).collect();
        let end = len.map(|n| (from + n) as usize).unwrap_or(whole.len());
        let mime = self.mime.lock().unwrap().clone();
        Ok(BlobRead {
            mime: if mime.is_empty() {
                "application/pdf".into()
            } else {
                mime
            },
            plain_len: whole.len() as u64,
            bytes: whole[from as usize..end.min(whole.len())].to_vec(),
            from,
            partial: from != 0 || len.is_some(),
        })
    }
}

struct Rig {
    app: axum::Router,
    port: Arc<FakePort>,
}

fn rig() -> Rig {
    rig_with(BlobLimits::DEFAULT)
}

fn rig_with(limits: BlobLimits) -> Rig {
    rig_full(limits, Arc::new(Access))
}

fn rig_full(limits: BlobLimits, access: Arc<dyn AccessChecker>) -> Rig {
    let port = Arc::new(FakePort::default());
    let state = ApiState {
        registry: Arc::new(RegistryHandle::new(Arc::new(
            MapRegistry::new([]).expect("empty registry"),
        ))),
        executor: Arc::new(NoExec),
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(ordo_api::MemoryBus::new()),
        replay: Arc::new(ordo_api::MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: ordo_api::PageKey::from_secret(b"k"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Rig {
        app: router_with_blobs(
            state,
            BlobState {
                port: Arc::clone(&port) as Arc<dyn BlobPort>,
                limits,
            },
        ),
        port,
    }
}

/// No ops in this rig — the blob routes do not reach the executor, and a
/// rig that could execute one would be testing the op path by accident.
struct NoExec;

impl ordo_api::spec::OpExecutor for NoExec {
    fn execute<'a>(
        &'a self,
        _op: &'a ordo_api::OpSpec,
        _space: &'a str,
        _bound: ordo_api::BoundParams,
        _refine: Option<ordo_api::Refinement>,
        _page: Option<ordo_api::PageCursor>,
        _lease: Option<ordo_api::ReplayLease>,
    ) -> futures::future::BoxFuture<'a, Result<ordo_api::ExecOutcome, ordo_api::ExecError>> {
        Box::pin(async { Err(ordo_api::ExecError::internal("no ops in this rig")) })
    }
}

fn put(uri: &str, len: Option<u64>, body: Vec<u8>) -> Request<Body> {
    let mut b = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .header("content-type", "application/pdf");
    if let Some(n) = len {
        b = b.header("content-length", n.to_string());
    }
    b.body(Body::from(body)).unwrap()
}

fn get(uri: &str, range: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE);
    if let Some(r) = range {
        b = b.header("range", r);
    }
    b.body(Body::empty()).unwrap()
}

const H: &str = "0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn an_upload_streams_through_and_is_recorded() {
    let r = rig();
    let body = vec![7u8; 5000];
    let res = r
        .app
        .clone()
        .oneshot(put(
            "/blob/documents/contract/doc_1",
            Some(5000),
            body.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let writes = r.port.writes.lock().unwrap();
    assert_eq!(writes.len(), 1);
    let (correlation, declared, mime, got) = &writes[0];
    assert_eq!(*declared, 5000);
    assert_eq!(*got, 5000, "the body reached the port in full");
    assert_eq!(mime, "application/pdf");
    // The correlation is DERIVED, never accepted: a caller who could
    // choose it would choose what their key use is attributed to.
    assert!(correlation.starts_with("blob.put:usr_ok:"), "{correlation}");
}

/// **The cap is enforced against `Content-Length`, before the body is
/// read** — which is the whole reason the length must be declared. The
/// class is `bound_violation`, which the door already has; a new
/// `blob_too_large` would be a new refusal class, and a new class is an
/// oracle until argued.
#[tokio::test]
async fn an_oversize_upload_is_refused_by_bound_violation() {
    let r = rig_with(BlobLimits { max_object: 1024 });
    let res = r
        .app
        .clone()
        .oneshot(put(
            "/blob/documents/contract/doc_1",
            Some(4096),
            vec![0; 8],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "bound_violation");
    // And the port was never reached, so no key was released and no object
    // was written.
    assert!(r.port.writes.lock().unwrap().is_empty());
}

/// A length discovered at the end is a cap enforced after the bytes
/// arrived, so an upload with no `Content-Length` is refused rather than
/// streamed.
#[tokio::test]
async fn an_upload_without_a_declared_length_is_refused() {
    let r = rig();
    let res = r
        .app
        .clone()
        .oneshot(put("/blob/documents/contract/doc_1", None, vec![1, 2, 3]))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(r.port.writes.lock().unwrap().is_empty());
}

/// **The authorization ruling, proved.** The check is made against the
/// row's own derived std op, so an entity whose op surface is closed has a
/// closed blob door too — the blob route cannot grant what the op surface
/// does not.
#[tokio::test]
async fn the_door_check_is_the_rows_own_std_op() {
    let r = rig();
    let res = r
        .app
        .clone()
        .oneshot(put("/blob/secrets/contract/doc_1", Some(3), vec![1, 2, 3]))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(r.port.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_request_without_a_token_or_a_space_never_reaches_the_port() {
    let r = rig();
    // No token.
    let res = r
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/blob/{H}"))
                .header("x-ordo-space", SPACE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // No space.
    let res = r
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/blob/{H}"))
                .header("authorization", "Bearer tok-ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(r.port.reads.lock().unwrap().is_empty());
}

/// A whole-object read: 200, the mime the platform recorded, and the two
/// headers that matter.
#[tokio::test]
async fn a_read_serves_the_object_with_the_headers_that_make_it_safe() {
    let r = rig();
    let res = r
        .app
        .clone()
        .oneshot(get(&format!("/blob/{H}"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let h = res.headers().clone();
    assert_eq!(h["content-type"], "application/pdf");
    // A handle is never reused, so the bytes behind one can never change.
    assert_eq!(h["cache-control"], "private, max-age=31536000, immutable");
    // The mime is a caller-supplied label; without this a browser can
    // decide an uploaded file was HTML after all, which is stored XSS.
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["content-disposition"], "inline");
    // A PDF is the one inline type not under `sandbox`: Chrome's viewer
    // shows a blank page under it. Nothing but the viewer loads.
    assert_eq!(
        h["content-security-policy"],
        "default-src 'none'; object-src 'self'"
    );
    assert_eq!(h["accept-ranges"], "bytes");
    assert!(
        h.get("content-range").is_none(),
        "a full read is not partial"
    );

    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(body.len(), 100);
}

/// **HTTP ranges are inclusive and the pipeline's are half-open**, and the
/// conversion is off by one in exactly one direction. Checked through the
/// route, so the door's arithmetic and the port's agree.
#[tokio::test]
async fn a_range_request_converts_and_answers_206() {
    let r = rig();
    let res = r
        .app
        .clone()
        .oneshot(get(&format!("/blob/{H}"), Some("bytes=10-19")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()["content-range"], "bytes 10-19/100");
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(body.len(), 10);
    assert_eq!(&body[..], &(10u8..20).collect::<Vec<u8>>()[..]);
    assert_eq!(*r.port.reads.lock().unwrap(), vec![(10, Some(10))]);
}

/// An unusable `Range` is a **full** response, which is what HTTP says —
/// not an error, and not a guess.
#[tokio::test]
async fn an_unusable_range_is_a_full_response() {
    let r = rig();
    let res = r
        .app
        .clone()
        .oneshot(get(&format!("/blob/{H}"), Some("bytes=-500")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(*r.port.reads.lock().unwrap(), vec![(0, None)]);
}

/// The metadata read answers the media type and the size as JSON, moves no
/// bytes, and refuses an absent handle as the read does.
#[tokio::test]
async fn the_meta_read_answers_type_and_size_and_nothing_else() {
    let r = rig();
    let res = r
        .app
        .clone()
        .oneshot(get(&format!("/blob/{H}/meta"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "application/json");
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v,
        serde_json::json!({ "mime": "application/pdf", "size": 100 })
    );
    assert!(
        r.port.reads.lock().unwrap().is_empty(),
        "a metadata read moves no bytes"
    );

    let res = r
        .app
        .clone()
        .oneshot(get(&format!("/blob/{}/meta", "0".repeat(32)), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// A handle that is not there is `not_found` — the same answer a handle in
/// another space gets, and the same answer a row the guard hides gets.
#[tokio::test]
async fn an_absent_handle_answers_not_found() {
    let r = rig();
    let res = r
        .app
        .clone()
        .oneshot(get(&format!("/blob/{}", "0".repeat(32)), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// **The op route's body limit is explicit now, and it is the op route's
/// alone.** Before this it was axum's implicit 2 MB default: operative,
/// untested and undocumented. A single layer over the whole router would
/// have capped uploads at the envelope's size — which is exactly why the
/// two routes are built as two `Router`s and merged.
#[tokio::test]
async fn the_op_body_limit_does_not_cap_the_blob_route() {
    assert_eq!(ordo_api::OP_BODY_LIMIT, 2 * 1024 * 1024);
    let r = rig();
    let big = 4 * 1024 * 1024u64;
    let res = r
        .app
        .clone()
        .oneshot(put(
            "/blob/documents/contract/doc_1",
            Some(big),
            vec![3u8; big as usize],
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NO_CONTENT,
        "a 4 MB upload must not be refused by the op envelope's 2 MB limit"
    );
    assert_eq!(r.port.writes.lock().unwrap()[0].3, big as usize);
}

/// A mime the caller supplies is a label, never a decision — but it
/// reaches a response header, so a value that could split one is dropped
/// rather than trusted.
#[tokio::test]
async fn a_hostile_content_type_is_replaced_rather_than_echoed() {
    let r = rig();
    let res = r
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/blob/documents/contract/doc_1")
                .header("authorization", "Bearer tok-ok")
                .header("x-ordo-space", SPACE)
                .header("content-length", "3")
                // A header value axum itself will accept, carrying a
                // 300-byte mime — longer than anything legitimate.
                .header("content-type", "x/".to_string() + &"a".repeat(300))
                .body(Body::from(vec![1, 2, 3]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        r.port.writes.lock().unwrap()[0].2,
        "application/octet-stream",
        "an out-of-bounds content type falls back rather than being stored"
    );
}

/// The type, disposition and policy a stored file of `mime` is served with.
async fn served(r: &Rig, mime: &str, token: bool) -> (StatusCode, [String; 4]) {
    *r.port.mime.lock().unwrap() = mime.to_string();
    let req = if token {
        get(&format!("/blob/{H}"), None)
    } else {
        Request::builder()
            .method("GET")
            .uri(format!("/blob/{H}"))
            .body(Body::empty())
            .unwrap()
    };
    let res = r.app.clone().oneshot(req).await.unwrap();
    let h = res.headers();
    let get = |k: &str| {
        h.get(k)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    };
    (
        res.status(),
        [
            get("content-type"),
            get("content-disposition"),
            get("content-security-policy"),
            get("x-content-type-options"),
        ],
    )
}

const CSP: &str = "sandbox; default-src 'none'";

/// A file uploaded as HTML or SVG would run its script on the door's origin
/// and could read a fresh token from `/auth/refresh`. It is saved, never
/// shown, and the stored type is left as it was.
#[tokio::test]
async fn a_file_claiming_a_scriptable_type_is_saved_never_shown() {
    let r = rig();
    for mime in [
        "text/html",
        "text/html; charset=utf-8",
        "TEXT/HTML",
        "image/svg+xml",
        "application/xhtml+xml",
        "text/xml",
        "application/xml",
        "application/x-unknown",
        "video/",
        "nonsense",
    ] {
        let (status, h) = served(&r, mime, true).await;
        assert_eq!(status, StatusCode::OK, "{mime}");
        assert_eq!(
            h,
            [
                "application/octet-stream".to_string(),
                "attachment".into(),
                CSP.into(),
                "nosniff".into()
            ],
            "{mime}"
        );
    }
}

/// The types a browser shows without script are served inline, as
/// themselves, still under the sandbox.
#[tokio::test]
async fn a_file_of_a_safe_type_is_shown_inline_under_the_sandbox() {
    let r = rig();
    for (mime, as_served) in [
        ("image/png", "image/png"),
        ("image/jpeg", "image/jpeg"),
        ("image/gif", "image/gif"),
        ("image/webp", "image/webp"),
        ("image/avif", "image/avif"),
        ("video/mp4", "video/mp4"),
        ("audio/mpeg", "audio/mpeg"),
        ("text/plain", "text/plain; charset=utf-8"),
        ("text/plain; charset=latin1", "text/plain; charset=utf-8"),
    ] {
        let (status, h) = served(&r, mime, true).await;
        assert_eq!(status, StatusCode::OK, "{mime}");
        assert_eq!(
            h,
            [
                as_served.to_string(),
                "inline".into(),
                CSP.into(),
                "nosniff".into()
            ],
            "{mime}"
        );
    }
}

/// A public file needs no token, so its URL alone is the attack. The
/// anonymous read gets exactly the headers a signed-in one does.
#[tokio::test]
async fn a_public_read_gets_the_same_headers() {
    let r = rig_full(BlobLimits::DEFAULT, Arc::new(OpenAccess));
    let (status, h) = served(&r, "text/html", false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h,
        [
            "application/octet-stream".to_string(),
            "attachment".into(),
            CSP.into(),
            "nosniff".into()
        ]
    );
    let (status, h) = served(&r, "image/png", false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h,
        [
            "image/png".to_string(),
            "inline".into(),
            CSP.into(),
            "nosniff".into()
        ]
    );
}

/// A video is seeked by ranges; the sandbox does not change that.
#[tokio::test]
async fn a_video_range_is_still_served() {
    let r = rig();
    *r.port.mime.lock().unwrap() = "video/mp4".into();
    let res = r
        .app
        .clone()
        .oneshot(get(&format!("/blob/{H}"), Some("bytes=10-19")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    let h = res.headers();
    assert_eq!(h["content-type"], "video/mp4");
    assert_eq!(h["content-disposition"], "inline");
    assert_eq!(h["content-range"], "bytes 10-19/100");
    assert_eq!(h["content-security-policy"], CSP);
}
