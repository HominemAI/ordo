//! `ordo fn runs|log --host …` (`plan-backend-functions.md` §10): the
//! deployment owner lists function runs and reads one run's log.
//!
//! The door here is a loopback stand-in: it checks the bearer, serves the
//! two run ops only as a `GET` in the deployment's own space, records the
//! path and query each call sent, and answers rows shaped as
//! `ordo_host::fn_runs` answers them (that shape is proved against Postgres
//! in `ordo-host/tests/functions.rs`). What this file proves is what the CLI
//! sends and prints.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::any;
use axum::{Json, Router};
mod support;
use ordo_identity::principal::{PrincipalId, Timestamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use support::operator;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"wire-fn-runs-token-key";
const HOME: &str = "ordo_home";
const OWNER: &str = "usr_owner";

fn token() -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(OWNER.into()),
            expires: Timestamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

type Seen = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;

fn run_row(seq: u64, function: &str) -> Value {
    json!({
        "seq": seq,
        "function": function,
        "version": "v-0123456789abcdef",
        "tenant": "acme",
        "wake": "call",
        "principal": format!("function:{function}@v-0123456789abcdef on behalf of ana"),
        // 2026-10-31 12:00:00 UTC.
        "startedAt": 1_793_448_000u64,
        "wallMs": 42,
        "outcome": "ok",
        "logLines": 2,
    })
}

async fn op(
    State(seen): State<Seen>,
    method: Method,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> axum::response::Response {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if ordo_identity::verify(&TokenKey::from_secret(SECRET), bearer, Timestamp(NOW)).is_err() {
        return refusal("access_denied", "not permitted");
    }
    if method != Method::GET {
        return refusal("method_mismatch", "a read is a GET");
    }
    if headers.get("x-ordo-space").and_then(|v| v.to_str().ok()) != Some(HOME) {
        return refusal(
            "not_found",
            "the run ops answer only in the deployment's own space",
        );
    }
    seen.lock().unwrap().push((name.clone(), query.clone()));
    match name.as_str() {
        "std.deployment.functionRuns" => {
            let rows = match query.get("function").map(String::as_str) {
                Some("nothing") => vec![],
                Some(f) => vec![run_row(8, f)],
                None => vec![run_row(9, "suggestTitle"), run_row(8, "chatty")],
            };
            Json(json!({ "rows": rows })).into_response()
        }
        "std.deployment.functionRun" => match query.get("seq").map(String::as_str) {
            Some("9") => {
                let mut row = run_row(9, "suggestTitle");
                let o = row.as_object_mut().unwrap();
                o.insert("cpuMs".into(), json!(3));
                o.insert("peakMemory".into(), json!(1_048_576));
                o.insert("fetches".into(), json!(1));
                o.insert("hosts".into(), json!(["api.anthropic.com"]));
                o.insert("bytesOut".into(), json!(120));
                o.insert("bytesIn".into(), json!(300));
                o.insert("message".into(), json!(""));
                o.insert(
                    "logs".into(),
                    json!([
                        {"level": "info", "text": "suggested a title of 22 characters"},
                        {"level": "warn", "text": "the far end said Bearer [secret]"},
                    ]),
                );
                Json(json!({ "rows": [row] })).into_response()
            }
            _ => refusal(
                "not_found",
                "no function run 77: runs are kept ORDO_FN_RUN_RETENTION_DAYS days",
            ),
        },
        _ => refusal("not_found", "no such op"),
    }
}

fn refusal(code: &str, message: &str) -> axum::response::Response {
    let status = if code == "not_found" {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::FORBIDDEN
    };
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

async fn serve() -> (std::net::SocketAddr, Seen) {
    let seen: Seen = Arc::default();
    let app = Router::new()
        .route("/op/{name}", any(op))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, seen)
}

fn args(addr: std::net::SocketAddr, head: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = head.iter().map(|s| s.to_string()).collect();
    a.extend([
        "--host".into(),
        format!("http://{addr}"),
        "--token".into(),
        token(),
        "--space".into(),
        HOME.into(),
    ]);
    a
}

#[test]
fn the_owner_lists_runs_and_reads_one_runs_log() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (addr, seen) = rt.block_on(serve());

    // Every run, newest first: when, function, version cut short, tenant,
    // outcome, duration, log lines and who.
    let out = operator::run("fn", &args(addr, &["runs"]));
    assert_eq!(out.code, 0, "{}", out.report);
    let lines: Vec<&str> = out.report.lines().collect();
    assert_eq!(lines.len(), 3, "{}", out.report);
    assert!(lines[0].starts_with("RUN  WHEN"), "{}", out.report);
    assert_eq!(
        lines[1],
        "9  2026-10-31 12:00:00Z  suggestTitle  v-0123456789  acme  ok  42  2  \
         function:suggestTitle@v-0123456789abcdef on behalf of ana"
    );
    assert!(lines[2].starts_with("8  "), "{}", out.report);

    // One function's runs, with a limit, sent as the op's query.
    let out = operator::run("fn", &args(addr, &["runs", "chatty", "--limit", "5"]));
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("chatty"), "{}", out.report);
    let out = operator::run("fn", &args(addr, &["runs", "nothing"]));
    assert_eq!(out.code, 0, "{}", out.report);
    assert_eq!(out.report, "no runs of `nothing` are kept\n");

    // One run: its measures and its log, a secret as `[secret]`.
    let out = operator::run("fn", &args(addr, &["log", "9"]));
    assert_eq!(out.code, 0, "{}", out.report);
    for want in [
        "run 9  suggestTitle@v-0123456789  in acme",
        "outcome  ok\n",
        "took     42 ms wall, 3 ms CPU",
        "fetches  1 (120 bytes out, 300 in) to api.anthropic.com",
        "  info   suggested a title of 22 characters",
        "  warn   the far end said Bearer [secret]",
    ] {
        assert!(out.report.contains(want), "{want:?} in\n{}", out.report);
    }

    // A run no longer kept: the door's answer, which names the setting.
    let out = operator::run("fn", &args(addr, &["log", "77"]));
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("ORDO_FN_RUN_RETENTION_DAYS"),
        "{}",
        out.report
    );

    let seen = seen.lock().unwrap().clone();
    let calls: Vec<(String, Vec<(String, String)>)> = seen
        .into_iter()
        .map(|(n, q)| {
            let mut q: Vec<_> = q.into_iter().collect();
            q.sort();
            (n, q)
        })
        .collect();
    let q = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    assert_eq!(
        calls,
        vec![
            ("std.deployment.functionRuns".into(), q(&[])),
            (
                "std.deployment.functionRuns".into(),
                q(&[("function", "chatty"), ("limit", "5")])
            ),
            (
                "std.deployment.functionRuns".into(),
                q(&[("function", "nothing")])
            ),
            ("std.deployment.functionRun".into(), q(&[("seq", "9")])),
            ("std.deployment.functionRun".into(), q(&[("seq", "77")])),
        ]
    );
}

#[test]
fn a_bad_argument_is_refused_before_anything_is_sent() {
    for (head, want) in [
        (vec!["runs", "--limit", "0"], "`--limit` is a whole number"),
        (vec!["runs", "a", "b"], "at most one function name"),
        (vec!["log"], "one run number"),
        (vec!["log", "seven"], "not a run number"),
        (vec!["tail"], "`runs` and `log`"),
    ] {
        let args: Vec<String> = head.iter().map(|s| s.to_string()).collect();
        let out = operator::run("fn", &args);
        assert_eq!(out.code, 2, "{head:?}: {}", out.report);
        assert!(out.report.contains(want), "{head:?}: {}", out.report);
    }
}
