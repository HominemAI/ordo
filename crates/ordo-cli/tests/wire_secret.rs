//! `ordo secret set|ls|rm --host …` (`plan-backend-functions.md` §5): the
//! deployment owner sets, lists and removes an app secret.
//!
//! The door here is a loopback stand-in in front of the real secret ops
//! over a memory store: it checks the bearer, asks presence for a change as
//! the deployment's door does by default, and serves a read only as a
//! `GET`. What this file proves is what the CLI sends and prints: the value
//! goes from standard input to the op and is never printed or listed.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::any;
use axum::{Json, Router};
use ordo_api::BoundParams;
mod support;
use ordo_host::secrets::{SecretOps, SecretStore};
use ordo_identity::principal::{PrincipalId, Timestamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use support::operator;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"wire-secret-token-key";
const HOME: &str = "ordo_home";
const OWNER: &str = "usr_owner";
const VALUE: &str = "sk-cli-0123456789";

fn token(present: bool) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(OWNER.into()),
            expires: Timestamp(NOW + 3_600),
            client: None,
            user_presence_at: present.then_some(Timestamp(NOW)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn op(
    State(ops): State<Arc<SecretOps>>,
    method: Method,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let space = headers
        .get("x-ordo-space")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let Ok(claims) = ordo_identity::verify(&TokenKey::from_secret(SECRET), bearer, Timestamp(NOW))
    else {
        return refusal("access_denied", "not permitted");
    };
    let read = name == ordo_host::secrets::LIST;
    if read != (method == Method::GET) {
        return refusal("method_mismatch", "a read is a GET and a change is a POST");
    }
    if !read && claims.user_presence_at.is_none() {
        return refusal("presence_required", "touch your passkey");
    }
    let mut params = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    params.remove("$space");
    match ops
        .execute(&name, &space, &BoundParams(params), &claims.principal.0)
        .await
    {
        Ok(r) => Json(json!({"rows": r.rows})).into_response(),
        Err(e) => refusal(e.code.as_str(), &e.message),
    }
}

fn refusal(code: &str, message: &str) -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

async fn serve() -> (std::net::SocketAddr, Arc<SecretOps>) {
    let store = SecretStore::memory(Arc::new(ordo_crypto::TestProvider::with_space(HOME)), HOME);
    store.bind(
        &ordo_syntax::parse_program("secret openaiKey for api.openai.com\n")
            .unwrap()
            .secrets,
    );
    let ops = Arc::new(SecretOps {
        store: Arc::new(store),
        now: Arc::new(|| NOW),
        stream: None,
    });
    let app = Router::new()
        .route("/op/{name}", any(op))
        .with_state(ops.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, ops)
}

fn args(addr: std::net::SocketAddr, tok: &str, head: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = head.iter().map(|s| s.to_string()).collect();
    a.extend([
        "--host".into(),
        format!("http://{addr}"),
        "--token".into(),
        tok.into(),
        "--space".into(),
        HOME.into(),
    ]);
    a
}

#[test]
fn the_owner_sets_lists_and_removes_a_secret_and_never_sees_its_value() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (addr, ops) = rt.block_on(serve());
    let input = format!("{VALUE}\n");

    // Without a touch the door refuses a change, and the CLI says how.
    let out = operator::run_with_input(
        "secret",
        &args(addr, &token(false), &["set", "openaiKey"]),
        input.as_bytes(),
    );
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("presence_required"), "{}", out.report);
    assert!(out.report.contains("ordo login --touch"), "{}", out.report);

    for version in [1, 2] {
        let out = operator::run_with_input(
            "secret",
            &args(addr, &token(true), &["set", "openaiKey"]),
            input.as_bytes(),
        );
        assert_eq!(out.code, 0, "{}", out.report);
        assert!(
            out.report.contains(&format!(
                "openaiKey version {version}; bound to api.openai.com"
            )),
            "{}",
            out.report
        );
        assert!(!out.report.contains(VALUE), "{}", out.report);
    }

    // The store holds version 2, set by the owner.
    let listed = rt.block_on(ops.store.list()).unwrap();
    assert_eq!((listed[0].version, listed[0].set_by.as_str()), (2, OWNER));

    // A listing is a read: no touch, and never a value.
    let out = operator::run("secret", &args(addr, &token(false), &["ls"]));
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("openaiKey  version 2"),
        "{}",
        out.report
    );
    assert!(
        out.report.contains("bound to api.openai.com"),
        "{}",
        out.report
    );
    assert!(!out.report.contains(VALUE), "{}", out.report);

    let out = operator::run("secret", &args(addr, &token(true), &["rm", "openaiKey"]));
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("REMOVED  secret openaiKey"),
        "{}",
        out.report
    );
    let out = operator::run("secret", &args(addr, &token(true), &["rm", "openaiKey"]));
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("std.deployment.secrets"),
        "{}",
        out.report
    );
    let out = operator::run("secret", &args(addr, &token(false), &["ls"]));
    assert!(out.report.contains("no secrets are set"), "{}", out.report);
}

#[test]
fn a_value_on_the_command_line_is_refused_before_anything_is_sent() {
    let out = operator::run(
        "secret",
        &[
            "set".into(),
            "openaiKey".into(),
            "--value".into(),
            VALUE.into(),
        ],
    );
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("standard input"), "{}", out.report);
    assert!(!out.report.contains(VALUE), "{}", out.report);

    // A second positional is a value typed where the name goes.
    let out = operator::run("secret", &["set".into(), "k".into(), VALUE.into()]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(!out.report.contains(VALUE), "{}", out.report);

    let out = operator::run_with_input("secret", &["set".into(), "k".into()], b"\n");
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("held no value"), "{}", out.report);

    let out = operator::run("secret", &["drop".into()]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(
        out.report.contains("`set`, `ls` and `rm`"),
        "{}",
        out.report
    );
}
