//! The CORS layers (`design-public-path.md` §8): the deployment's list, a
//! tenant's own list, and the wildcard a public answer carries.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use ordo_api::cors::{apply_layers, Cors, CorsPolicy, PublicAnswer, PublicOps, TenantOrigins};
use tower::util::ServiceExt;

/// `shop` lists `https://shop.example`; `blog` lists `https://blog.example`.
struct Lists;

impl TenantOrigins for Lists {
    fn allows(&self, space: &str, origin: &str) -> bool {
        matches!(
            (space, origin),
            ("shop", "https://shop.example") | ("blog", "https://blog.example")
        )
    }
    fn any_allows(&self, origin: &str) -> bool {
        origin == "https://shop.example" || origin == "https://blog.example"
    }
}

/// `public` is the one public op.
struct Public;

impl PublicOps for Public {
    fn is_public(&self, op: &str) -> bool {
        op == "public"
    }
}

async fn private() -> Response {
    "members only".into_response()
}

async fn public() -> Response {
    let mut r = "everyone".into_response();
    r.extensions_mut().insert(PublicAnswer);
    r
}

fn app(deployment: Option<&str>, tenants: bool) -> Router {
    let router = Router::new()
        .route("/op/private", get(private).post(private))
        .route("/op/public", get(public).post(public));
    apply_layers(
        router,
        Cors {
            deployment: deployment.map(|d| CorsPolicy::parse(d).unwrap().unwrap()),
            tenants: tenants.then(|| Arc::new(Lists) as Arc<dyn TenantOrigins>),
            public_ops: Some(Arc::new(Public)),
        },
    )
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap) {
    let r = app.clone().oneshot(req).await.unwrap();
    (r.status(), r.headers().clone())
}

fn acao(h: &axum::http::HeaderMap) -> &str {
    h.get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

fn actual(path: &str, origin: &str, space: Option<&str>) -> Request<Body> {
    let mut b = Request::get(path)
        .header("origin", origin)
        .header("authorization", "Bearer t");
    if let Some(s) = space {
        b = b.header("x-ordo-space", s);
    }
    b.body(Body::empty()).unwrap()
}

fn preflight(origin: &str, headers: &str) -> Request<Body> {
    preflight_to("/op/private", origin, headers)
}

fn preflight_to(path: &str, origin: &str, headers: &str) -> Request<Body> {
    Request::builder()
        .method("OPTIONS")
        .uri(path)
        .header("origin", origin)
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", headers)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn a_tenants_origin_reads_its_own_space_and_no_other() {
    let app = app(Some("https://app.example"), true);
    let (_, h) = send(
        &app,
        actual("/op/private", "https://shop.example", Some("shop")),
    )
    .await;
    assert_eq!(acao(&h), "https://shop.example");
    let (_, h) = send(
        &app,
        actual("/op/private", "https://shop.example", Some("blog")),
    )
    .await;
    assert_eq!(acao(&h), "", "another tenant's answer is not readable");
    let (_, h) = send(&app, actual("/op/private", "https://shop.example", None)).await;
    assert_eq!(acao(&h), "", "no space named, the deployment's list alone");
    // The deployment's list holds for every space.
    let (_, h) = send(
        &app,
        actual("/op/private", "https://app.example", Some("blog")),
    )
    .await;
    assert_eq!(acao(&h), "https://app.example");
}

#[tokio::test]
async fn a_preflight_answers_the_union_and_only_tells_an_origin_about_itself() {
    let app = app(Some("https://app.example"), true);
    let (s, h) = send(
        &app,
        preflight("https://shop.example", "authorization, x-ordo-space"),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(acao(&h), "https://shop.example");
    let (s, h) = send(&app, preflight("https://evil.example", "authorization")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(acao(&h), "");
}

#[tokio::test]
async fn with_no_tenant_lists_a_tenant_origin_is_not_allowed() {
    let app = app(Some("https://app.example"), false);
    let (_, h) = send(
        &app,
        actual("/op/private", "https://shop.example", Some("shop")),
    )
    .await;
    assert_eq!(acao(&h), "");
}

#[tokio::test]
async fn a_public_answer_is_readable_from_any_page_with_no_list_at_all() {
    let app = app(None, false);
    let req = Request::get("/op/public")
        .header("origin", "https://anyone.example")
        .body(Body::empty())
        .unwrap();
    let (_, h) = send(&app, req).await;
    assert_eq!(acao(&h), "*");
    // A private answer gets nothing.
    let (_, h) = send(&app, actual("/op/private", "https://anyone.example", None)).await;
    assert_eq!(acao(&h), "");
}

#[tokio::test]
async fn a_public_preflight_is_answered_star_and_never_allows_a_token() {
    let app = app(None, false);
    let (s, h) = send(
        &app,
        preflight_to(
            "/op/public",
            "https://anyone.example",
            "content-type, x-ordo-human",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(acao(&h), "*");
    let allowed = h
        .get("access-control-allow-headers")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    assert!(allowed.contains("x-ordo-human"), "{allowed}");
    assert!(!allowed.contains("authorization"), "{allowed}");
    // Asking to send a token is not a public call.
    let (s, h) = send(
        &app,
        preflight_to(
            "/op/public",
            "https://anyone.example",
            "Authorization, content-type",
        ),
    )
    .await;
    assert_ne!(s, StatusCode::NO_CONTENT);
    assert_eq!(acao(&h), "");
}

/// ID-F1: a bare preflight (no request headers) to a members' op from an
/// origin on no list is checked against the list, never answered `*`.
#[tokio::test]
async fn a_bare_preflight_to_a_members_op_is_checked_against_the_list() {
    for (deployment, tenants) in [(Some("https://app.example"), true), (None, false)] {
        let app = app(deployment, tenants);
        let bare = Request::builder()
            .method("OPTIONS")
            .uri("/op/private")
            .header("origin", "https://evil.test")
            .header("access-control-request-method", "GET")
            .body(Body::empty())
            .unwrap();
        let (s, h) = send(&app, bare).await;
        assert_ne!(acao(&h), "*", "{deployment:?}");
        assert_ne!(s, StatusCode::NO_CONTENT, "{deployment:?}");
    }
    // The listed page still gets its own origin back.
    let app = app(Some("https://app.example"), false);
    let ok = Request::builder()
        .method("OPTIONS")
        .uri("/op/private")
        .header("origin", "https://app.example")
        .header("access-control-request-method", "GET")
        .body(Body::empty())
        .unwrap();
    let (s, h) = send(&app, ok).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(acao(&h), "https://app.example");
}

/// ID-F2: every response through the layer, a preflight and a refusal
/// included, carries the hardening headers.
#[tokio::test]
async fn every_response_carries_the_hardening_headers() {
    let app = app(Some("https://app.example"), false);
    let requests = [
        actual("/op/private", "https://app.example", None),
        actual("/op/missing", "https://app.example", None),
        preflight("https://evil.test", "authorization"),
        preflight("https://app.example", "authorization"),
        Request::get("/op/public").body(Body::empty()).unwrap(),
    ];
    for req in requests {
        let path = req.uri().to_string();
        let (_, h) = send(&app, req).await;
        for (name, value) in [
            ("x-content-type-options", "nosniff"),
            ("x-frame-options", "DENY"),
            ("referrer-policy", "no-referrer"),
        ] {
            assert_eq!(
                h.get(name).and_then(|v| v.to_str().ok()),
                Some(value),
                "{path}: {name}"
            );
        }
    }
}
