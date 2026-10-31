//! `ordo console-key revoke --host …` (`plan-console.md` 8.2): the
//! deployment owner revokes a console's key from the deployment's side.
//!
//! The door here is a loopback stand-in in front of the real console key
//! ops over a memory store: it checks the bearer and its presence, as the
//! deployment's door does for this op (never exempt from step-up), then
//! runs the op. What this file proves is what the CLI sends and prints.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use ordo_api::BoundParams;
mod support;
use ordo_host::console_keys::{ConsoleKeyOps, ConsoleKeyStore, ConsoleKeys, KeyRow};
use ordo_identity::principal::{PrincipalId, Timestamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use support::operator;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"wire-console-key-secret";
const HOME: &str = "ordo_home";
const OWNER: &str = "usr_owner";
const KEY: &str = "ck-0123456789abcdef0123456789abcdef";

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
    State(ops): State<Arc<ConsoleKeyOps>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
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
    if claims.user_presence_at.is_none() {
        return refusal("step_up_required", "touch your passkey");
    }
    let mut params = body.as_object().cloned().unwrap_or_default();
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

async fn serve() -> (std::net::SocketAddr, Arc<ConsoleKeyOps>) {
    let store = ConsoleKeyStore::memory();
    store
        .insert(&KeyRow {
            id: KEY.into(),
            principal: OWNER.into(),
            name: "ops console".into(),
            public_key: "pk".into(),
            created_at: NOW,
            last_used_at: None,
            revoked_at: None,
            revoked_by: None,
        })
        .await
        .unwrap();
    store.signed_in(KEY, "s1", NOW + 900, NOW).await.unwrap();
    let ops = Arc::new(ConsoleKeyOps {
        keys: Arc::new(ConsoleKeys::new(store, [1; 32], 900, HOME)),
        revocations: Some(Arc::new(ordo_identity::MemorySessionRevocations::new())),
        now: Arc::new(|| NOW),
        stream: None,
    });
    let app = Router::new()
        .route("/op/{name}", post(op))
        .with_state(ops.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, ops)
}

fn revoke(addr: std::net::SocketAddr, tok: &str, id: &str) -> operator::Outcome {
    operator::run(
        "console-key",
        &[
            "revoke".into(),
            "--id".into(),
            id.into(),
            "--host".into(),
            format!("http://{addr}"),
            "--token".into(),
            tok.into(),
            "--space".into(),
            HOME.into(),
        ],
    )
}

#[test]
fn the_owner_revokes_a_console_key_and_its_sessions_end() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (addr, ops) = rt.block_on(serve());

    // Without a touch the door refuses, and the CLI says why.
    let out = revoke(addr, &token(false), KEY);
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("step_up_required"), "{}", out.report);

    let out = revoke(addr, &token(true), KEY);
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains(KEY), "{}", out.report);
    assert!(
        out.report.contains("1 open session(s) ended"),
        "{}",
        out.report
    );
    let row = rt.block_on(ops.keys.store.get(KEY)).unwrap().unwrap();
    assert_eq!(row.revoked_by.as_deref(), Some(OWNER));

    // A second revocation names the op that lists the keys.
    let out = revoke(addr, &token(true), KEY);
    assert_ne!(out.code, 0);
    assert!(out.report.contains("consoleKeys"), "{}", out.report);

    // The subcommand and its flags are checked before anything is sent.
    let out = operator::run("console-key", &["drop".into()]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("revoke"), "{}", out.report);
}
