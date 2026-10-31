//! 14g: the CORS allowlist layer, over a router shaped like the door.
//!
//! The routes mirror the door's `/op/{name}` shape. `ordo-host`'s
//! `tests/supervisor.rs` runs the same layer over the real assembled door.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use ordo_api::cors::{apply, CorsPolicy};
use tower::util::ServiceExt;

const APP: &str = "https://app.example.com";

fn door() -> Router {
    Router::new().route("/op/{name}", get(|| async { "rows" }))
}

fn allowlisted() -> Router {
    let policy = CorsPolicy::parse(&format!("{APP},http://localhost:5173")).unwrap();
    assert!(policy.is_some());
    apply(door(), policy)
}

fn preflight(origin: &str) -> Request<Body> {
    Request::builder()
        .method("OPTIONS")
        .uri("/op/listReadings")
        .header("origin", origin)
        .header("access-control-request-method", "GET")
        .header(
            "access-control-request-headers",
            "authorization, x-ordo-space, idempotency-key",
        )
        .body(Body::empty())
        .unwrap()
}

fn get_from(origin: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method("GET").uri("/op/listReadings");
    if let Some(o) = origin {
        b = b.header("origin", o);
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn a_preflight_from_an_allowed_origin_succeeds() {
    let resp = allowlisted().oneshot(preflight(APP)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let h = resp.headers();
    assert_eq!(h["access-control-allow-origin"], APP);
    let methods = h["access-control-allow-methods"].to_str().unwrap();
    for m in ["GET", "POST", "PUT", "DELETE"] {
        assert!(methods.contains(m), "{methods}");
    }
    let allowed = h["access-control-allow-headers"]
        .to_str()
        .unwrap()
        .to_ascii_lowercase();
    for name in ["authorization", "x-ordo-space", "idempotency-key"] {
        assert!(allowed.contains(name), "{allowed}");
    }
    assert!(h.contains_key("access-control-max-age"));
    assert!(h["vary"].to_str().unwrap().contains("Origin"));
    assert!(!h.contains_key("access-control-allow-credentials"));
}

#[tokio::test]
async fn a_preflight_from_another_origin_is_refused() {
    let resp = allowlisted()
        .oneshot(preflight("https://evil.example.net"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(!resp.headers().contains_key("access-control-allow-origin"));
    assert!(!resp.headers().contains_key("access-control-allow-methods"));
}

#[tokio::test]
async fn a_real_request_from_an_allowed_origin_carries_the_header() {
    let resp = allowlisted().oneshot(get_from(Some(APP))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["access-control-allow-origin"], APP);
    assert!(resp.headers()["vary"].to_str().unwrap().contains("Origin"));
}

#[tokio::test]
async fn a_real_request_from_another_origin_gets_no_allow_origin() {
    let resp = allowlisted()
        .oneshot(get_from(Some("https://evil.example.net")))
        .await
        .unwrap();
    assert!(!resp.headers().contains_key("access-control-allow-origin"));
    assert!(resp.headers()["vary"].to_str().unwrap().contains("Origin"));
}

#[tokio::test]
async fn a_request_with_no_origin_is_served_as_before() {
    let resp = allowlisted().oneshot(get_from(None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!resp.headers().contains_key("access-control-allow-origin"));
}

#[tokio::test]
async fn with_the_setting_absent_nothing_changes() {
    let policy = CorsPolicy::parse("").unwrap();
    assert!(policy.is_none());
    let app = apply(door(), policy);
    let resp = app.clone().oneshot(preflight(APP)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(!resp.headers().contains_key("access-control-allow-origin"));
    let resp = app.oneshot(get_from(Some(APP))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!resp.headers().contains_key("access-control-allow-origin"));
    assert!(!resp.headers().contains_key("vary"));
}

#[tokio::test]
async fn a_wildcard_does_not_parse() {
    assert!(CorsPolicy::parse("*").is_err());
}

/// The renewal cookie (owner ruling 2026-10-21): `/auth/` answers allow
/// credentials for a listed origin, and nothing else does.
#[tokio::test]
async fn credentials_are_allowed_on_auth_routes_for_listed_origins_only() {
    let policy = CorsPolicy::parse(APP).unwrap();
    let app = apply(
        Router::new()
            .route("/op/{name}", get(|| async { "rows" }))
            .route("/auth/refresh", axum::routing::post(|| async { "token" })),
        policy,
    );
    let refresh = |origin: &str| {
        Request::builder()
            .method("POST")
            .uri("/auth/refresh")
            .header("origin", origin)
            .body(Body::empty())
            .unwrap()
    };
    let resp = app.clone().oneshot(refresh(APP)).await.unwrap();
    assert_eq!(resp.headers()["access-control-allow-origin"], APP);
    assert_eq!(resp.headers()["access-control-allow-credentials"], "true");

    let resp = app
        .clone()
        .oneshot(refresh("https://evil.example.net"))
        .await
        .unwrap();
    assert!(!resp
        .headers()
        .contains_key("access-control-allow-credentials"));
    assert!(!resp.headers().contains_key("access-control-allow-origin"));

    let pre = Request::builder()
        .method("OPTIONS")
        .uri("/auth/refresh")
        .header("origin", APP)
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(pre).await.unwrap();
    assert_eq!(resp.headers()["access-control-allow-credentials"], "true");

    let resp = app.clone().oneshot(get_from(Some(APP))).await.unwrap();
    assert!(!resp
        .headers()
        .contains_key("access-control-allow-credentials"));
}
