//! Defect 25ax: sign-in routes were mounted on the router the in-process
//! tests use (`auth::router`) and not on the one the `ordo-host` binary
//! serves (`serve::platform_router`), so a real server answered 404.
//!
//! The fix is one list: both routers call `auth::auth_routes`. These tests
//! keep it one list. They need no database.

use std::path::Path;

use axum::body::Body;
use axum::http::Request;
use tower::util::ServiceExt;

fn source(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Code lines only: comments name routes without mounting them.
fn code(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `"/auth/..."` or `"/.well-known/webauthn"` literal in the auth
/// sources that sits in a `.route(` call.
fn mounted_paths() -> Vec<String> {
    let mut files = vec![source("src/auth.rs")];
    for e in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/auth")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            files.push(std::fs::read_to_string(p).unwrap());
        }
    }
    let mut out = Vec::new();
    for f in files {
        let c = code(&f);
        let mut rest = c.as_str();
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + 7..];
            if let Some(t) = rest.trim_start().strip_prefix('"') {
                if let Some(end) = t.find('"') {
                    let path = &t[..end];
                    if path.starts_with("/auth/") || path == "/.well-known/webauthn" {
                        out.push(path.to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn the_served_router_takes_its_auth_routes_from_the_shared_list() {
    let serve = code(&source("src/serve.rs"));
    assert!(
        serve.contains("crate::auth::auth_routes(auth)"),
        "serve::platform_router must merge auth::auth_routes(auth)"
    );
    for path in mounted_paths() {
        assert!(
            !serve.contains(&format!("\"{path}\"")),
            "serve.rs mounts {path} by hand; put it in auth::auth_routes so both routers have it"
        );
    }
    let auth = code(&source("src/auth.rs"));
    assert!(
        auth.contains("auth_routes(auth.clone()).with_state(auth)"),
        "auth::router must be auth_routes() and nothing else"
    );
}

#[tokio::test]
async fn every_auth_path_in_the_sources_is_mounted_on_the_shared_list() {
    let paths = mounted_paths();
    assert!(paths.len() > 40, "found only {} paths", paths.len());
    for path in paths {
        let uri = path.replace("{connection}", "x").replace("{tenant}", "t");
        let mut mounted = false;
        for method in ["GET", "POST"] {
            let res = ordo_host::auth::router(None)
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(&uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            // A mounted route answers the other method 405, never 404.
            mounted |= res.status() != 404;
        }
        assert!(mounted, "{path} is not mounted");
    }
}
