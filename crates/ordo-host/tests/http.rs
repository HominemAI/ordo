//! Stage 7 end-to-end: HTTP request → door → bind → compile → Postgres →
//! envelope, with the write path landing in the command log.
//!
//! Everything the earlier stages proved piecewise is exercised through the
//! one public surface: the guarded read narrows per bearer, the write
//! protocol's refusals arrive as their typed wire errors (`not_found` 404,
//! `access_guard` 403), and the door's denial does not leak op existence.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::{FixtureExecutor, MemberAccess, DEFAULT_SPACE};
use serde_json::Value;
use tower::util::ServiceExt;

async fn state(db: &str) -> (ApiState, Pg) {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    let deployment = ordo_host::deploy::Deployment::pinned(
        "http-test",
        f.clone(),
        ordo_exec::Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(FixtureExecutor {
            spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
            pg: pg.clone(),
            page_size: 2,
        }),
        access: Arc::new(MemberAccess {
            fixture: f,
            pg: pg.clone(),
        }),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"ordo-host-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    (state, pg)
}

async fn call(
    state: &ApiState,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-ordo-space", DEFAULT_SPACE);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => req
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

#[tokio::test]
async fn the_engine_serves_the_fixture_end_to_end() {
    let (state, pg) = state("ordo_host").await;

    // ── a guarded read narrows per bearer ───────────────────────────────
    let (st, body) = call(&state, "GET", "/op/openTasks", Some("member:mem_1"), None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let titles: Vec<&str> = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["audit books", "draft spec"],
        "Ana's two readable projects, title-sorted"
    );

    // Bram spans proj_1 + proj_2 — three rows, which the deployment's page
    // size (2) splits, so the first page is full and carries a cursor.
    let (_, body) = call(&state, "GET", "/op/openTasks", Some("member:mem_2"), None).await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 2);
    let next = body["next"].as_str().expect("a full page mints a cursor");
    let (_, body) = call(
        &state,
        "GET",
        &format!("/op/openTasks?$page={}", urlencode(next)),
        Some("member:mem_2"),
        None,
    )
    .await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 1, "the tail page");
    assert!(
        body["next"].is_null(),
        "a partial page is the end of the order"
    );

    // ── the door: bad token, and no op-existence oracle ─────────────────
    let (st, body) = call(&state, "GET", "/op/openTasks", Some("member:mem_x"), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    let denied_real = body["error"]["code"].clone();
    let (st2, body2) = call(&state, "GET", "/op/noSuchOp", Some("member:mem_x"), None).await;
    assert_eq!(
        (st, denied_real),
        (st2, body2["error"]["code"].clone()),
        "a denied caller cannot distinguish a real op from a fiction"
    );

    // ── a parameterized read through the wire ───────────────────────────
    let (st, body) = call(
        &state,
        "GET",
        "/op/customerByEmail?e=ada@example.com",
        Some("member:mem_1"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"][0]["name"], "Ada",
        "blind-index equality over HTTP"
    );

    // ── writes: method mirror, typed refusals, real effects ─────────────
    let (st, body) = call(
        &state,
        "POST",
        "/op/renameTask",
        Some("member:mem_1"),
        Some(serde_json::json!({ "task": "task_1", "title": "spec v3", "$space": DEFAULT_SPACE })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let row = pg
        .query(r#"SELECT "title", "rev" FROM "tasks" WHERE "id" = 'task_1'"#)
        .unwrap();
    assert_eq!(row[0][0].as_deref(), Some("spec v3"));
    assert_eq!(row[0][1].as_deref(), Some("2"), "rev bumped through HTTP");

    // GET on a write op is refused before anything runs.
    let (st, _) = call(&state, "GET", "/op/renameTask", Some("member:mem_1"), None).await;
    assert_eq!(st, StatusCode::METHOD_NOT_ALLOWED);

    // Bram reads task_1 but cannot write it: 403 access_guard.
    let (st, body) = call(
        &state,
        "POST",
        "/op/renameTask",
        Some("member:mem_2"),
        Some(serde_json::json!({ "task": "task_1", "title": "nope", "$space": DEFAULT_SPACE })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "access_guard");

    // Dov cannot even see it: 404, same as a fiction.
    let (st, body) = call(
        &state,
        "POST",
        "/op/renameTask",
        Some("member:mem_4"),
        Some(serde_json::json!({ "task": "task_1", "title": "nope", "$space": DEFAULT_SPACE })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found");
}

/// Paging over HTTP: the cursor is an opaque token, it reassembles the
/// whole result exactly once, and it is bound to the principal it was
/// minted for.
#[tokio::test]
async fn pages_are_opaque_principal_bound_and_complete() {
    let (state, _pg) = state("ordo_host_page").await;

    // Page size 2 (set in `state`), so allTasks spans several pages.
    let mut seen: Vec<String> = Vec::new();
    let mut url = "/op/allTasks".to_string();
    let mut tokens: Vec<String> = Vec::new();
    for _ in 0..10 {
        let (st, body) = call(&state, "GET", &url, Some("member:mem_1"), None).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let rows = body["rows"].as_array().unwrap();
        assert!(rows.len() <= 2, "the deployment's page size governs");
        seen.extend(
            rows.iter()
                .map(|r| r["title"].as_str().unwrap().to_string()),
        );
        match body["next"].as_str() {
            Some(t) => {
                tokens.push(t.to_string());
                url = format!("/op/allTasks?$page={}", urlencode(t));
            }
            None => break,
        }
    }
    // allTasks is unguarded over all five seeded tasks, title-sorted.
    assert_eq!(
        seen,
        [
            "audit books",
            "build site",
            "draft spec",
            "review spec",
            "ship site"
        ],
        "every row once, in the plan's order"
    );

    // The token is opaque freight — no sort key legible in it.
    let t = &tokens[0];
    assert!(
        !t.contains("audit") && !t.contains("build"),
        "cursor leaks its keyset: {t}"
    );

    // And it is bound to its principal: another member presenting it is
    // refused rather than seeked.
    let (st, body) = call(
        &state,
        "GET",
        &format!("/op/allTasks?$page={}", urlencode(t)),
        Some("member:mem_2"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_cursor");
}

fn urlencode(s: &str) -> String {
    s.replace('+', "%2B")
        .replace('/', "%2F")
        .replace('=', "%3D")
}

/// 9g: the write path's typed refusals arrive on the wire as themselves.
///
/// Each of these rules is *built* — the tree maintains itself, deletes
/// refuse with referrers — and each one used to arrive as `internal`, 500,
/// which is what the platform says when it has a bug. A caller could not
/// tell "you asked for a cycle" from "the server fell over", and the two
/// want opposite responses.
///
/// The status is the second half of the answer and it is not uniform. All
/// three below are **409**: the *stored data* refused them, and the same
/// request lands once that data changes — reparent the child, repoint the
/// product. That is what a conflict is. A refusal no data could satisfy
/// (`tree_bulk_reparent`, `reserved_derived_name`) is 400 instead; the
/// unit tests in `ordo-api::error` hold that line for the classes this
/// fixture has no op to reach.
#[tokio::test]
async fn typed_write_refusals_arrive_typed_and_not_as_500() {
    let (state, pg) = state("ordo_host_refuse").await;
    // Seeded: cat_1 goods → cat_2 tools → cat_3 handtools, and products
    // pointing at cat_2..cat_5.

    // A move that would put `tools` inside its own subtree.
    let (st, body) = call(
        &state,
        "POST",
        "/op/moveCategory",
        Some("member:mem_1"),
        Some(serde_json::json!({ "cat": "cat_2", "parent": "cat_3", "$space": DEFAULT_SPACE })),
    )
    .await;
    assert_eq!(body["error"]["code"], "tree_cycle", "{body}");
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty()),
        "the class travels with its reason: {body}"
    );

    // Deleting a node that still has children.
    let (st, body) = call(
        &state,
        "DELETE",
        "/op/dropCategory?cat=cat_2",
        Some("member:mem_1"),
        None,
    )
    .await;
    assert_eq!(body["error"]["code"], "tree_has_children", "{body}");
    assert_eq!(st, StatusCode::CONFLICT, "{body}");

    // Deleting a row another row still references — the platform's own
    // vocabulary, never the database's constraint name.
    let (st, body) = call(
        &state,
        "DELETE",
        "/op/dropCategory?cat=cat_3",
        Some("member:mem_1"),
        None,
    )
    .await;
    assert_eq!(body["error"]["code"], "has_referrers", "{body}");
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("products.category"),
        "names the referrer: {msg}"
    );
    assert!(
        !msg.contains("constraint"),
        "and does not leak the physical schema: {msg}"
    );

    // The refused writes changed nothing: `handtools` is still there, and
    // still under `tools`. A typed refusal aborts its whole group.
    let row = pg
        .query(r#"SELECT "parent" FROM "categories" WHERE "id" = 'cat_3'"#)
        .unwrap();
    assert_eq!(row[0][0].as_deref(), Some("cat_2"), "nothing applied");
}

/// 9q: `$caller` is the platform's to bind, and a client that supplies one
/// is refused at the door — on reads and writes alike, before any guard is
/// evaluated against the principal it proposed.
#[tokio::test]
async fn a_client_supplied_caller_is_refused_at_the_door() {
    let (state, _pg) = state("ordo_host_caller").await;

    for (method, path, body) in [
        (
            "POST",
            "/op/renameTask",
            Some(serde_json::json!({
                "task": "task_1", "title": "x", "$caller": "mem_9", "$space": DEFAULT_SPACE
            })),
        ),
        ("GET", "/op/openTasks?$caller=mem_9", None),
    ] {
        let (st, out) = call(&state, method, path, Some("member:mem_1"), body).await;
        assert_eq!(
            out["error"]["code"], "caller_supplied",
            "{method} {path}: {out}"
        );
        assert_eq!(st, StatusCode::FORBIDDEN, "{out}");
        assert_eq!(out["error"]["param"], "$caller");
    }

    // And the op still works without it — the refusal is about the key,
    // not about the request being otherwise malformed.
    let (st, body) = call(
        &state,
        "POST",
        "/op/renameTask",
        Some("member:mem_1"),
        Some(serde_json::json!({ "task": "task_1", "title": "x", "$space": DEFAULT_SPACE })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

/// The tripwire standing in for a typed write-path error (see
/// `ordo_host::classify`'s hazard note). The write protocol and the pooled
/// executor encode their refusal class in a **string prefix**; nothing in
/// the type system makes that prefix real. So read their source and insist
/// that every prefix they raise is either a class the wire inventory knows
/// or one of the handful that are deliberately `internal`.
///
/// What this catches: a new typed refusal whose class nobody added to
/// `ErrorCode`, and a misspelled prefix. What it does not: a refusal that
/// carries no prefix at all. Only changing those `String`s to a typed
/// error would catch that, which is the fix this test is standing in for.
#[test]
fn every_raised_class_prefix_is_a_wire_class() {
    // The two request-path tiers below the transport.
    const SOURCES: &[(&str, &str)] = &[
        (
            "ordo-conformance/src/write.rs",
            include_str!("../../ordo-conformance/src/write.rs"),
        ),
        (
            "ordo-exec/src/lib.rs",
            include_str!("../../ordo-exec/src/lib.rs"),
        ),
    ];
    // Deliberately internal (500), and right to be: a *registered* op that
    // fails to parse, bind, or plan at request time is a server bug, and a
    // broken connection (`db`) is not the caller's to act on. A full pool
    // is, and it raises `server_busy:` (25z), which this test now sees.
    // Prefixes that are genuinely the server's fault and belong as 500s.
    // `seal` joined this list when key custody landed: a payload that will
    // not seal is a custody fault — a missing space, an unreachable
    // provider — and there is nothing the caller did or can do about it.
    // (This test caught that addition the same day it was made, which is
    // the whole reason it exists.)
    // `unscoped_write` joined it with the space column (9h): it is the
    // write path's tripwire for a statement that names a table without
    // confining it to the request's space. A caller cannot cause one and
    // has nothing to do about it — it means the platform was about to
    // read or write another tenant's rows, which is a server fault of the
    // most serious kind and a 500 is the right answer. What matters is
    // that it is raised at all; where it lands on the wire is secondary.
    // `batch_actor_changed` and `batch_flag_changed` joined it with bulk
    // import: a batch stages many groups under one correlation, and every
    // group in one batch carries one actor and one flag set. Changing
    // either mid-batch is a fault in whatever drives the batch, not
    // something a caller can reach: import runs through the library, and
    // no wire op opens a batch. If a batch ever becomes wire-reachable
    // these move to `ordo_api::ErrorCode` with a status, and this test is
    // what will say so.
    // `lock_timeout` joined it with 27d: only a statement that sets
    // `lock_timeout` can raise it, and the two that do never let it reach
    // a caller raw. The schema apply retries it and reports its own
    // `ExecFailure::LockTimeout`; the apply op's lock turns it into
    // `apply_in_progress` (`apply_op::ApplyLock::take`). Seen at the wire it
    // would be a server fault.
    const INTERNAL: &[&str] = &[
        "lock_timeout",
        "parse",
        "bind",
        "plan",
        "internal",
        "db",
        "seal",
        "unscoped_write",
        "batch_actor_changed",
        "batch_flag_changed",
        // Only a walk over the tenant databases waits on the dial cap; no
        // request does, so this never reaches a caller.
        "dial_wait_exceeded",
    ];

    let mut checked = 0usize;
    for (name, src) in SOURCES {
        for prefix in class_prefixes(src) {
            if INTERNAL.contains(&prefix.as_str()) {
                continue;
            }
            let classified = ordo_host::classify(format!("{prefix}: something happened"));
            assert_ne!(
                classified.code.as_str(),
                "internal",
                "`{name}` raises `{prefix}:` but the wire inventory has no such class, so it \
                 reaches the caller as a 500 — add it to `ordo_api::ErrorCode` (with a status), \
                 or add it to this test's INTERNAL list if it is genuinely a server fault"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 15,
        "the scanner found only {checked} class prefixes — it has stopped seeing them"
    );

    // The tripwire has teeth: a class the inventory does not know — a
    // typo, or one nobody added — is exactly what falls through to 500.
    assert_eq!(
        ordo_host::classify("tree_cyle: a misspelling".into())
            .code
            .as_str(),
        "internal"
    );
    assert_eq!(
        ordo_host::classify("tree_cycle: spelled right".into())
            .code
            .as_str(),
        "tree_cycle"
    );
    // And the door's own word is not the executor's to say, however it
    // arrives: `access_denied` from below is a bug, not a 403.
    assert_eq!(
        ordo_host::classify("access_denied: not yours".into())
            .code
            .as_str(),
        "internal"
    );
}

/// String literals of the form `"<lower_snake>:` — the write path's class
/// convention.
fn class_prefixes(src: &str) -> Vec<String> {
    let bytes: Vec<char> = src.chars().collect();
    let mut out: Vec<String> = Vec::new();
    for (i, c) in bytes.iter().enumerate() {
        if *c != '"' {
            continue;
        }
        let mut word = String::new();
        for ch in &bytes[i + 1..] {
            if ch.is_ascii_lowercase() || *ch == '_' {
                word.push(*ch);
            } else {
                if *ch == ':' && !word.is_empty() && !out.contains(&word) {
                    out.push(word);
                }
                break;
            }
        }
    }
    out
}

/// Stage 8 — ops truth. The manifest is derived from the same artifacts
/// the executor runs, so it cannot drift from behaviour.
#[tokio::test]
async fn the_manifest_tells_the_truth_about_each_op() {
    let f = Fixture::load().expect("fixture");

    // A guarded read: cost from the classifier, guards with provenance.
    let m = ordo_host::manifest(&f, "noteFeed").expect("registered");
    assert_eq!(m["kind"], "read");
    assert_eq!(m["cost"]["class"], "sql");
    assert_eq!(m["unbounded"], true);
    let guard = m["guards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["entity"] == "taskNotes")
        .expect("taskNotes guard disclosed");
    assert_eq!(
        guard["source"], "inherited from tasks",
        "an inherited guard names where it came from"
    );
    assert!(
        guard["read"].as_str().unwrap().contains("task."),
        "and renders rebased through the owning path: {}",
        guard["read"]
    );

    // Composition reaches through callees: specCount sources specTasks →
    // openTasks → tasks, and the manifest discloses the entity and guard
    // the caller never typed.
    let m = ordo_host::manifest(&f, "specCount").expect("registered");
    assert_eq!(
        m["entities"],
        serde_json::json!(["projectMembers", "projects", "tasks"]),
        "the guard's own entities are part of the exposure, not a detail"
    );
    assert_eq!(m["cost"]["rows"], "<= 1", "reduce proves one row");
    assert_eq!(m["guards"][0]["entity"], "projectMembers");

    // R-G8: the unguarded flag never travels without naming what it strips.
    let m = ordo_host::manifest(&f, "allTasks").expect("registered");
    assert_eq!(m["unguarded"], true);
    assert_eq!(m["bypassed_guards"], serde_json::json!(["tasks.read"]));

    // A write op carries its derived mode, and its entities are the ones
    // its statements touch PLUS the ones its guards examine to decide
    // whether the write may land.
    let m = ordo_host::manifest(&f, "addTask").expect("registered");
    assert_eq!(m["kind"], "write");
    assert_eq!(m["mode"], "creates");
    assert_eq!(
        m["entities"],
        serde_json::json!(["projectMembers", "projects", "tasks"])
    );
    let m = ordo_host::manifest(&f, "purgeDone").expect("registered");
    assert_eq!(m["kind"], "delete");
    assert_eq!(
        m["mode"], "bulk deletes",
        "a ranged delete is not the same disclosure as an addressed one, \
         and the label is where a reviewer learns which this is"
    );

    // §6 rules 1 and 8: the mode is derived from identity precedence in
    // the braces, and it is what a reviewer reads instead of re-deriving
    // it. The three unconditioned `set`s below differ ONLY in what their
    // braces say about identity.
    let mode = |op: &str| ordo_host::manifest(&f, op).expect("registered")["mode"].clone();
    assert_eq!(
        mode("recordEntry"),
        "creates-or-converges",
        "one covered unique key is the blessed upsert"
    );
    assert_eq!(
        mode("recordEntryOnce"),
        "creates",
        "`rev: null` cannot converge, and the label says so — that is the \
         whole point of spelling create-once"
    );
    assert_eq!(
        mode("renumberEntry"),
        "converges",
        "`id` in the braces addresses; `externalId` is carried as a value"
    );
    assert_eq!(
        mode("claimCoupon"),
        "ambiguous-identity",
        "two unique keys is not a mode, it is a refusal"
    );
    // Rule 1's fourth label, which the old three-way heuristic could not
    // say at all: a condition that is not `id ==` ranges, and every matched
    // row is written.
    assert_eq!(mode("movePM"), "converges", "`id == $pm` addresses one row");
    assert_eq!(
        mode("dropFolderTree"),
        "deletes subtree",
        "a cascade removes descendants the caller never addressed"
    );
    // The list rule 1 requires beside the union: one entry per statement,
    // in statement order, each naming its entity and verb. Every op in the
    // fixture is single-statement, so this is the shape assertion.
    //
    // Two keys ride beside rule 1's own three, and both are about what the
    // CALLER sees rather than what the compiler decided: `summary` is what
    // this statement puts in the response (a count, here — a converge does
    // not return the row), and `cas_param` is the parameter it asserts as
    // the row's revision. `closeTask($task)` asserts none, which is why the
    // op's `call.cas` is false and why a `cas_miss` affordance would be one
    // it can never raise.
    let m = ordo_host::manifest(&f, "closeTask").expect("registered");
    assert_eq!(
        m["statements"],
        serde_json::json!([{
            "entity": "tasks",
            "verb": "set",
            "label": "converges",
            "summary": { "keys": ["updated"], "value": "count" },
            "cas_param": serde_json::Value::Null,
        }]),
    );
    assert_eq!(
        ordo_host::manifest(&f, "renameTaskIf").expect("registered")["statements"][0]["cas_param"],
        "rev",
        "`set tasks(id == $task) {{ …, rev: $rev }}` is where CAS is declared, \
         and naming the parameter is what saves a runtime from finding it by name"
    );
    assert_eq!(
        ordo_host::manifest(&f, "openTasks").expect("registered")["statements"],
        serde_json::json!([]),
        "a read has no statements — an empty list, never a missing key"
    );

    // The case a single word cannot state: an op that CREATES one row and
    // CONVERGES another. The union is both labels, in statement order, and
    // the list says which statement is which. A three-way heuristic over
    // the whole op had to pick one of them and be wrong about the other.
    let mut mixed = f.in_space(ordo_host::DEFAULT_SPACE);
    mixed.registry.extend_from(
        &ordo_syntax::parse_program(
            "op transferDemo($ext: text, $who: id, $amt: money, $task: id) =\n  \
             set ledgerEntries { externalId: $ext, owner: $who, amount: $amt }\n  \
             set tasks(id == $task) { status: \"done\" }\n",
        )
        .expect("parses"),
    );
    let m = ordo_host::manifest(&mixed, "transferDemo").expect("registered");
    assert_eq!(m["mode"], "creates-or-converges + converges");
    assert_eq!(
        m["statements"],
        serde_json::json!([
            {
                "entity": "ledgerEntries",
                "verb": "set",
                "label": "creates-or-converges",
                // Two possible keys, because the identity precedence decides
                // at run time which branch ran — and the caller learns which
                // from the key it gets back, not from the label.
                "summary": { "keys": ["created", "converged"], "value": "id" },
                "cas_param": serde_json::Value::Null,
            },
            {
                "entity": "tasks",
                "verb": "set",
                "label": "converges",
                "summary": { "keys": ["updated"], "value": "count" },
                "cas_param": serde_json::Value::Null,
            },
        ]),
        "the response shape is per STATEMENT for the same reason the label is: \
         one word over the whole op would be wrong about at least one half"
    );
    // And its scope is the union of both statements' — a transfer discloses
    // what both halves touch.
    let ents: Vec<&str> = m["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    assert!(
        ents.contains(&"ledgerEntries") && ents.contains(&"tasks"),
        "{ents:?}"
    );

    // An acl-derived guard says so, rather than looking hand-written.
    let m = ordo_host::manifest(&f, "cardList").expect("registered");
    let cards_guard = m["guards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["entity"] == "cards")
        .expect("cards' guard disclosed");
    assert_eq!(cards_guard["source"], "inherited from boards");

    // The catalog covers every registered op, and nothing else.
    let c = ordo_host::catalog(&f);
    assert_eq!(
        c.as_array().unwrap().len(),
        f.registry.names().count(),
        "the catalog is the registry, not a curated subset"
    );
}

/// 9l — the manifest says WHICH FIELDS.
///
/// The assertions are exact set equality, not `contains`: the whole value
/// of a field-scope manifest is that a caller consenting to it consents to
/// exactly this and no more, and a subset test would pass for a manifest
/// that quietly grew a column.
///
/// The case a naive implementation misses is `noteFeed`: `tasks.project` is
/// read by the guard walk (`task.project.projectMembers`) and by nothing
/// else — the op projects `body` and `task`, the caller never names
/// `tasks`, and a manifest built from the source text would not mention it.
/// It is disclosure all the same: the platform reads rows of `tasks` and
/// `projectMembers` on the caller's behalf to decide which notes they see.
#[tokio::test]
async fn the_manifest_says_which_fields_including_the_ones_only_a_guard_reads() {
    let f = Fixture::load().expect("fixture");
    // `entity -> (read fields, written fields, via)`, as the manifest states it.
    let scope = |op: &str| -> Vec<(String, Vec<String>, Vec<String>, String)> {
        ordo_host::manifest(&f, op).expect("registered")["scope"]
            .as_array()
            .expect("a scope array")
            .iter()
            .map(|s| {
                let list = |k: &str| {
                    s[k].as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().to_string())
                        .collect::<Vec<_>>()
                };
                (
                    s["entity"].as_str().unwrap().to_string(),
                    list("read"),
                    list("written"),
                    s["via"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let s = |e: &str, r: &[&str], w: &[&str], v: &str| {
        (
            e.to_string(),
            r.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
            w.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
            v.to_string(),
        )
    };

    // guards-conformance GC080, exactly: `openTasks` →
    // `tasks.{status, title, project}` + `projectMembers.{project, member}`
    // as guard reads. `canWrite` is absent — the READ guard never touches
    // it, which is the post-pruning discipline the document asks for.
    // `projects` is in the walk but no column of it is read; the semi-join
    // asks only whether the row exists, and the manifest says exactly that
    // rather than pretending the whole row was read or hiding the hop.
    assert_eq!(
        scope("openTasks"),
        vec![
            s("projectMembers", &["member", "project"], &[], "guard"),
            s("projects", &[], &[], "guard"),
            s("tasks", &["project", "status", "title"], &[], "body"),
        ]
    );

    // GC082, exactly: the depth-2 rebased guard. `tasks.{project}` is
    // reached ONLY through the guard — the op never names `tasks`.
    assert_eq!(
        scope("noteFeed"),
        vec![
            s("projectMembers", &["member", "project"], &[], "guard"),
            s("projects", &[], &[], "guard"),
            s("taskNotes", &["body", "task"], &[], "body"),
            s("tasks", &["project"], &[], "guard"),
        ]
    );

    // composition-conformance C116: composition flattens, and the caller's
    // projection PRUNES. `activeProducts` outputs `vendor`; `cheapActive`
    // does not project it, so the flattened plan never reads it and the
    // manifest must not claim it does.
    assert_eq!(
        scope("cheapActive"),
        vec![s(
            "products",
            &["active", "category", "name", "price"],
            &[],
            "body"
        )]
    );
    assert!(
        scope("activeProducts")[0].1.contains(&"vendor".to_string()),
        "the callee does read it — pruning is the caller's projection, not a bug"
    );

    // C117: depth 3, and a tree traversal reads STRUCTURE, not values.
    assert_eq!(
        scope("toolDeals"),
        vec![
            s("categories", &[], &[], "traversal"),
            s(
                "products",
                &["active", "category", "name", "price"],
                &[],
                "body"
            ),
        ]
    );
    assert_eq!(
        ordo_host::manifest(&f, "toolDeals").expect("registered")["scope"][0]["tree_structure"],
        true,
        "`under` reads the materialized path — shape, no value fields"
    );

    // A write states what it writes and what it reads to decide, and they
    // are different lists. GC081: `closeTask` converges one field, and the
    // guard reads `projectMembers.{project, member, canWrite}` — canWrite
    // appears here and not in `openTasks` because the WRITE guard is the
    // one that tests it.
    assert_eq!(
        scope("closeTask"),
        vec![
            s(
                "projectMembers",
                &["canWrite", "member", "project"],
                &[],
                "guard"
            ),
            s("projects", &[], &[], "guard"),
            s("tasks", &["project"], &["status"], "body"),
        ]
    );

    // A bulk delete reads the fields its condition names, and reaches the
    // rows the deleted rows own (`taskNotes`), whose guards it examines
    // (owned-delete, 2026-09-29).
    assert_eq!(
        scope("purgeDone"),
        vec![
            s(
                "projectMembers",
                &["canWrite", "member", "project"],
                &[],
                "guard"
            ),
            s("projects", &[], &[], "guard"),
            s("taskNotes", &["task"], &[], "guard"),
            s("tasks", &["project", "status"], &[], "body"),
        ]
    );

    // `unguarded` strips the guard, and the scope shrinks to match: the
    // flag's whole meaning is that those rows are no longer examined.
    assert_eq!(
        scope("allTasks"),
        vec![s("tasks", &["status", "title"], &[], "body")]
    );

    // The prose rendering carries the same fields, and says which of them
    // the caller never asked for.
    let prose = ordo_host::catalog_prose(&ordo_host::catalog(&f));
    assert!(
        prose.contains("reaches: tasks.{project, status, title}"),
        "{prose}"
    );
    assert!(
        prose
            .contains("reaches: projectMembers.{member, project} — read by a guard, not by the op"),
        "the consent page has to say the guard reads rows the caller never named"
    );
}

/// 9l — the release set: which keys a run needs, decided by the planner.
#[tokio::test]
async fn the_release_set_names_the_keys_a_run_will_need() {
    let f = Fixture::load().expect("fixture");
    let release = |op: &str| ordo_host::manifest(&f, op).expect("registered")["release"].clone();

    // Most ops need no key at all, and the manifest says so rather than
    // leaving it to be inferred from an absent list.
    assert_eq!(
        release("openTasks"),
        serde_json::json!({ "purpose": null, "keys": [] })
    );

    // A host-class read: the sealed projection is opened after the rows
    // come back, under a READ lease, into the named output column.
    assert_eq!(
        release("orderMemos"),
        serde_json::json!({
            "purpose": "read",
            "keys": [{
                "entity": "orders", "field": "memo",
                "key": "dek", "stage": "host", "output": "memo",
            }],
        })
    );

    // A blind-index read needs a DIFFERENT key at a DIFFERENT moment: the
    // per-field BIK, before the statement is sent. `host_open` carries
    // neither, which is why the release set is more than a rename of it.
    assert_eq!(
        release("customerByEmail"),
        serde_json::json!({
            "purpose": "read",
            "keys": [{
                "entity": "customers", "field": "email",
                "key": "blind-index", "stage": "sql", "output": null,
            }],
        })
    );

    // A write seals the value and digests the key it addresses by — two
    // keys, one statement, and a write plan has no `host_open` at all.
    assert_eq!(
        release("upsertCustomer"),
        serde_json::json!({
            "purpose": "write",
            "keys": [
                {
                    "entity": "customers", "field": "email",
                    "key": "blind-index", "stage": "write", "output": null,
                },
                {
                    "entity": "customers", "field": "email",
                    "key": "dek", "stage": "write", "output": null,
                },
            ],
        })
    );

    // A delete opens nothing: the row goes, envelope and all.
    assert_eq!(release("purgeNotes")["purpose"], serde_json::Value::Null);

    // DL §8: release follows the manifest. Every key an op will use is
    // named by the document the caller consented to — so the set of
    // (entity, field) pairs in the release set is a subset of the scope.
    for (name, m) in ordo_host::manifests(&f) {
        let in_scope: Vec<String> = m["scope"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| {
                let e = s["entity"].as_str().unwrap().to_string();
                s["read"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .chain(s["written"].as_array().unwrap())
                    .map(move |f| format!("{e}.{}", f.as_str().unwrap()))
                    .collect::<Vec<_>>()
            })
            .collect();
        for k in m["release"]["keys"].as_array().unwrap() {
            let pair = format!(
                "{}.{}",
                k["entity"].as_str().unwrap(),
                k["field"].as_str().unwrap()
            );
            assert!(
                in_scope.contains(&pair),
                "`{name}` releases a key for `{pair}`, which its manifest does not \
                 disclose — release is derived from op access and nothing else"
            );
        }
    }
}

// ── real credentials ─────────────────────────────────────────────────────

use ordo_identity::principal::{
    CapabilityGrant, ManifestHash, PrincipalId, SpaceId as ISpace, Timestamp,
};
use ordo_identity::{TokenKey, TokenSpec};

const NOW: u64 = 1_000_000;

async fn signed_state(db: &str) -> (ApiState, std::sync::Arc<ordo_host::IdentityAccess>) {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let access = std::sync::Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"deployment-secret"),
        now: Box::new(|| NOW),
        stream: std::sync::Arc::new(ordo_secevent::Recorder::default()),
        metrics: std::sync::Arc::new(ordo_host::metrics::Metrics::new()),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let deployment = ordo_host::deploy::Deployment::pinned(
        "http-test",
        f.clone(),
        ordo_exec::Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(FixtureExecutor {
            spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
            pg: pg.clone(),
            page_size: 50,
        }),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"ordo-host-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    (state, access)
}

fn token(principal: &str, expires: u64) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(b"deployment-secret"),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(expires),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

#[tokio::test]
async fn signed_credentials_open_the_door_and_nothing_else_does() {
    let (state, access) = signed_state("ordo_host_creds").await;

    // A real token for a real member reads, and the principal the engine
    // resolved is the one the guards bind — Ana's two projects, not
    // everyone's.
    let t = token("mem_1", NOW + 60);
    let (st, body) = call(&state, "GET", "/op/openTasks", Some(&t), None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let titles: Vec<&str> = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["audit books", "draft spec"]);

    // The old asserted form is not a credential any more.
    let (st, _) = call(&state, "GET", "/op/openTasks", Some("member:mem_1"), None).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "an unsigned claim proves nothing"
    );

    // Expiry is enforced against the checker's clock, not the token's word.
    let (st, _) = call(
        &state,
        "GET",
        "/op/openTasks",
        Some(&token("mem_1", NOW - 1)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // A token for a principal with no membership: same answer as a forgery.
    let (st, _) = call(
        &state,
        "GET",
        "/op/openTasks",
        Some(&token("mem_ghost", NOW + 60)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Every refusal was one wire answer; the REASONS exist, but only in
    // the operator's audit trail.
    let audit = access.take_audit();
    assert!(
        audit.iter().any(|a| a.contains("NoMembership")),
        "the operator sees why: {audit:?}"
    );
    assert!(
        audit.iter().any(|a| a.contains("TokenUnverifiable")),
        "and which failures never even named a principal: {audit:?}"
    );
    // 7a: the unverifiable presentation is recorded as `unverified` and
    // routed to the deployment, never to the space it named — a stranger
    // cannot grow a victim space's audit spine by guessing an id.
    let unverified: Vec<&String> = audit
        .iter()
        .filter(|a| a.contains("TokenUnverifiable"))
        .collect();
    assert!(
        unverified.iter().all(|a| a.contains("unverified")
            && a.contains("in deployment")
            && !a.contains("user:")),
        "an unverifiable token must claim no principal: {unverified:?}"
    );
}

#[tokio::test]
async fn suspension_stops_a_valid_token_immediately() {
    let (mut state, _access) = signed_state("ordo_host_susp").await;
    let t = token("mem_1", NOW + 600);

    let (st, _) = call(&state, "GET", "/op/openTasks", Some(&t), None).await;
    assert_eq!(st, StatusCode::OK);

    // Suspend, without touching the token: it is still perfectly signed
    // and years from expiry.
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg {
        db: "ordo_host_susp".into(),
        ..Pg::default()
    };
    let mut engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    engine.suspend(
        &ISpace(ordo_host::DEFAULT_SPACE.into()),
        &PrincipalId("mem_1".into()),
    );
    let access = std::sync::Arc::new(ordo_host::IdentityAccess {
        engine: std::sync::Arc::new(engine),
        key: TokenKey::from_secret(b"deployment-secret"),
        now: Box::new(|| NOW),
        stream: std::sync::Arc::new(ordo_secevent::Recorder::default()),
        metrics: std::sync::Arc::new(ordo_host::metrics::Metrics::new()),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });
    state.access = access.clone();

    let (st, _) = call(&state, "GET", "/op/openTasks", Some(&t), None).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "the same token stops working the instant status flips"
    );
    assert!(
        access.take_audit().iter().any(|a| a.contains("Suspended")),
        "and the operator's record says so, not merely 'denied'"
    );
}

#[tokio::test]
async fn a_capability_token_is_confined_and_dies_when_its_scope_changes() {
    let (state, access) = signed_state("ordo_host_cap").await;
    let f = Fixture::load().expect("fixture");

    let mut ops = std::collections::BTreeSet::new();
    ops.insert("openTasks".to_string());
    let pins = ordo_host::grant_pins(&f, &ops).expect("deployed manifests");

    let cap = |pins: std::collections::BTreeMap<String, ordo_identity::OpPin>| {
        ordo_identity::mint(
            &TokenKey::from_secret(b"deployment-secret"),
            &TokenSpec {
                principal: PrincipalId("mem_1".into()),
                expires: Timestamp(NOW + 600),
                client: None,
                user_presence_at: None,
                capability: Some(CapabilityGrant::new(
                    ordo_identity::GrantId("gr_test".into()),
                    ISpace(ordo_host::DEFAULT_SPACE.into()),
                    pins,
                    Timestamp(NOW + 600),
                )),
                delegation: None,
                session: None,
            },
        )
    };

    // Inside the allowlist: allowed, and still guard-narrowed — a grant
    // widens nothing about which ROWS the principal may see.
    let t = cap(pins.clone());
    let (st, body) = call(&state, "GET", "/op/openTasks", Some(&t), None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 2, "Ana's rows only");

    // Outside it: refused, though the same principal's ordinary token
    // would be allowed — the grant IS the whole authority.
    let (st, _) = call(&state, "GET", "/op/noteFeed", Some(&t), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(
        &state,
        "GET",
        "/op/noteFeed",
        Some(&token("mem_1", NOW + 60)),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "the membership path still works");

    // A grant pinning a manifest the deployment no longer serves is dead —
    // this is the Stage-8 join: consent was to a disclosed scope, so the
    // scope changing revokes it rather than silently widening it.
    let stale = cap(pins
        .iter()
        .map(|(op, p)| {
            (
                op.clone(),
                ordo_identity::OpPin {
                    manifest_hash: ManifestHash([0u8; 32]),
                    // A pin naming a field the served op does not read:
                    // an expansion, and therefore fatal. A bare hash
                    // change is no longer enough to kill a grant, which
                    // is the point of the ruling — so this test now has
                    // to state a real widening rather than a stale
                    // digest.
                    entities: vec![ordo_identity::EntityPin {
                        entity: "tasks".into(),
                        read: std::collections::BTreeSet::new(),
                        ..ordo_identity::EntityPin::new("tasks")
                    }],
                    ..p.clone()
                },
            )
        })
        .collect());
    let (st, _) = call(&state, "GET", "/op/openTasks", Some(&stale), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(
        access
            .take_audit()
            .iter()
            .any(|a| a.contains("ManifestChanged")),
        "the grant died for the right reason"
    );
}

/// 9l's consequence for consent, stated as a test: **a capability grant is
/// pinned to what the manifest DISCLOSED, so widening the disclosure kills
/// the grant — and narrowing it does not.**
///
/// This is not an unfortunate side effect of changing the manifest's shape
/// — it is the property that makes ops truth load-bearing. A grant said
/// "you may call `openTasks`, which reads `tasks.{project, status}`". An op
/// that now also reads `tasks.title` is a different disclosure, and the
/// caller consented to the old one. It dies, and a human re-consents.
///
/// # Both directions, because 8s made them different answers
///
/// This test used to assert only that a grant pinned to a manifest the
/// deployment does not serve is refused, and it minted the grant against the
/// **wider** program. Under a single hash that was the same question — any
/// difference denied — so the direction of the difference did not matter and
/// nothing here recorded which way round it was.
///
/// It matters now. A grant pinned to a wider manifest, presented to a
/// deployment serving a narrower one, is a person who consented to MORE than
/// they are being given, and DL §8 and `security-logging-design.md` §5 agree
/// that narrowing **applies silently**. So the second half below asserts a
/// **200** where this test used to assert 403, and that change of answer is
/// the ruling, not a regression: a rule that denies in both directions is
/// indistinguishable from the fail-closed combined hash it replaced.
#[tokio::test]
async fn widening_what_an_op_discloses_kills_the_grants_pinned_to_it() {
    let (state, access) = signed_state("ordo_host_manifest_pin").await;
    let f = Fixture::load().expect("fixture");

    let mut ops = std::collections::BTreeSet::new();
    ops.insert("openTasks".to_string());
    let served = ordo_host::grant_pins(&f, &ops).expect("deployed manifests");

    // The same program with one FEWER field in `openTasks`' disclosure. The
    // deployment under test serves the shipped program, so pinning against
    // this one is how a grant comes to name less than what is served.
    //
    // The sort key moves with the projection, because 9l's manifest is at
    // FIELD grain and a sort reads its key: dropping `title` from the
    // projection alone would leave `sort title` reading it, and the scope
    // would not move at all. That is the manifest being right — a column the
    // plan touches is disclosed whether or not it is returned — and it is
    // why this edit is two lines.
    let narrower_src = ordo_conformance::FIXTURE.replace(
        "  tasks(status == \"open\")[title, project]\n  sort title",
        "  tasks(status == \"open\")[project]\n  sort project",
    );
    assert_ne!(narrower_src, ordo_conformance::FIXTURE, "the edit applied");
    let program = ordo_syntax::parse_program(&narrower_src).expect("parses");
    let keys = ordo_schema::KeyTable::from_env().expect("keys");
    let mut narrower = f.in_space(ordo_host::DEFAULT_SPACE);
    narrower.schema = ordo_schema::build_with_keys(&program, &keys).expect("schema");
    narrower.registry = ordo_sema::Registry::from_program(&program);

    let less = ordo_host::grant_pins(&narrower, &ops).expect("deployed manifests");
    assert_ne!(
        served, less,
        "a narrower field scope must move the pin, or consent is to \
         a document that no longer describes the op"
    );
    // The field the edit removed is exactly what moved it.
    let scope = |fx: &Fixture| {
        ordo_host::manifest(fx, "openTasks").expect("registered")["scope"][2]["read"].clone()
    };
    assert_eq!(scope(&f), serde_json::json!(["project", "status", "title"]));
    assert_eq!(scope(&narrower), serde_json::json!(["project", "status"]));

    let mint = |pins: std::collections::BTreeMap<String, ordo_identity::OpPin>| {
        ordo_identity::mint(
            &TokenKey::from_secret(b"deployment-secret"),
            &TokenSpec {
                principal: PrincipalId("mem_1".into()),
                expires: Timestamp(NOW + 600),
                client: None,
                user_presence_at: None,
                capability: Some(CapabilityGrant::new(
                    ordo_identity::GrantId("gr_test".into()),
                    ISpace(ordo_host::DEFAULT_SPACE.into()),
                    pins,
                    Timestamp(NOW + 600),
                )),
                delegation: None,
                session: None,
            },
        )
    };

    // ── expansion: dead ──────────────────────────────────────────────────
    //
    // The grant names `tasks.{project, status}`; the deployment reads
    // `title` as well. That is a column this caller never consented to, and
    // the classification — not merely the inequality — is what kills it.
    assert!(
        matches!(
            ordo_identity::compare(&less["openTasks"], &served["openTasks"]),
            ordo_identity::PinDrift::Expanded { .. }
        ),
        "one more disclosed field is more reach, not less"
    );
    let (st, _) = call(&state, "GET", "/op/openTasks", Some(&mint(less)), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(
        access
            .take_audit()
            .iter()
            .any(|a| a.contains("ManifestChanged")),
        "and for the right reason: the manifest, not the credential"
    );

    // ── narrowing: alive ─────────────────────────────────────────────────
    //
    // The mirror image, minted the other way round. Every input the door
    // sees is different from the deployment's, and the call goes through,
    // because being given less than you consented to is not a reason to ask
    // somebody again.
    let mut wider = served.clone();
    wider.get_mut("openTasks").unwrap().entities[2]
        .read
        .insert("assignee".into());
    assert!(
        matches!(
            ordo_identity::compare(&wider["openTasks"], &served["openTasks"]),
            ordo_identity::PinDrift::Narrowed { .. }
        ),
        "a deployment reading one field fewer than the consent named is narrowing"
    );
    let (st, body) = call(&state, "GET", "/op/openTasks", Some(&mint(wider)), None).await;
    assert_eq!(st, StatusCode::OK, "narrowing applies silently: {body}");
}

/// F6: the catalog is access-checked like everything else — a token sees
/// exactly the ops it may call, and a capability token sees only its
/// allowlist. An agent's catalog is its consent page in machine form.
#[tokio::test]
async fn the_catalog_is_scoped_to_the_asker() {
    let (_state, access) = signed_state("ordo_host_catalog").await;
    let f = Fixture::load().expect("fixture");
    let names = |c: &Value| -> Vec<String> {
        c.as_array()
            .unwrap()
            .iter()
            .map(|m| m["op"].as_str().unwrap().to_string())
            .collect()
    };

    // A member sees the ops their role allows.
    let ana = ordo_host::catalog_for(
        &f,
        access.as_ref(),
        &token("mem_1", NOW + 60),
        ordo_host::DEFAULT_SPACE,
    );
    let ana_names = names(&ana);
    assert!(ana_names.contains(&"openTasks".to_string()));
    assert!(ana_names.len() > 20, "a full member sees the surface");

    // A capability token sees ONLY its allowlist — the catalog cannot
    // enumerate what the door would refuse.
    let mut ops = std::collections::BTreeSet::new();
    ops.insert("openTasks".to_string());
    let pins = ordo_host::grant_pins(&f, &ops).expect("deployed");
    let cap = ordo_identity::mint(
        &TokenKey::from_secret(b"deployment-secret"),
        &TokenSpec {
            principal: PrincipalId("mem_1".into()),
            expires: Timestamp(NOW + 600),
            client: None,
            user_presence_at: None,
            capability: Some(CapabilityGrant::new(
                ordo_identity::GrantId("gr_test".into()),
                ISpace(ordo_host::DEFAULT_SPACE.into()),
                pins,
                Timestamp(NOW + 600),
            )),
            delegation: None,
            session: None,
        },
    );
    assert_eq!(
        names(&ordo_host::catalog_for(
            &f,
            access.as_ref(),
            &cap,
            ordo_host::DEFAULT_SPACE
        )),
        ["openTasks"],
        "the grant IS the catalog"
    );

    // A token that opens nothing gets an empty catalog, not a listing with
    // everything marked forbidden — a name is a disclosure.
    let ghost = ordo_host::catalog_for(
        &f,
        access.as_ref(),
        &token("mem_ghost", NOW + 60),
        ordo_host::DEFAULT_SPACE,
    );
    assert!(ghost.as_array().unwrap().is_empty());

    // The prose rendering carries the same content, including the guard
    // provenance and the bypass warning.
    let prose = ordo_host::catalog_prose(&ana);
    assert!(prose.contains("## openTasks("), "{prose}");
    assert!(prose.contains("guard on tasks (own)"), "{prose}");
    assert!(
        prose.contains("WARNING: this op bypasses row guards"),
        "an unguarded op must say so to an agent too"
    );
}

/// A download source with nothing marked: the download is off.
struct NothingMarked;

impl ordo_api::download::DownloadSource for NothingMarked {
    fn ops(&self) -> Vec<ordo_api::download::DownloadOp> {
        Vec::new()
    }
    fn record(&self, _: &str, _: &str, _: Option<&str>, _: &[String]) {}
}

async fn get_download(app: &axum::Router, token: Option<&str>) -> StatusCode {
    let mut req = Request::get("/download").header("x-ordo-space", DEFAULT_SPACE);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

/// The refusals the door makes before `check` reach the security stream
/// too (board: denials not recorded): an op nobody registered with a valid
/// token, a call with no token, and `/download` refused because it is off
/// or for want of a token. Each answer on the wire is unchanged.
#[tokio::test]
async fn refusals_before_the_check_are_recorded() {
    let (state, access) = signed_state("ordo_host_refusals").await;
    let _ = access.take_audit();
    let t = token("mem_1", NOW + 60);

    // An unregistered op, valid token: the uniform 403, recorded with the
    // principal against the space it named.
    let (st, body) = call(&state, "GET", "/op/noSuchOp", Some(&t), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "access_denied");
    let audit = access.take_audit();
    let hit: Vec<&String> = audit
        .iter()
        .filter(|a| a.contains("UnregisteredOp"))
        .collect();
    assert_eq!(hit.len(), 1, "{audit:?}");
    assert!(!hit[0].contains("in deployment"), "{hit:?}");

    // No token at all: recorded unverified, in the deployment's stream.
    let (st, _) = call(&state, "GET", "/op/openTasks", None, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(&state, "GET", "/op/noSuchOp", None, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let audit = access.take_audit();
    let missing: Vec<&String> = audit
        .iter()
        .filter(|a| a.contains("TokenMissing"))
        .collect();
    assert_eq!(missing.len(), 2, "{audit:?}");
    assert!(
        missing
            .iter()
            .all(|a| a.contains("in deployment") && a.contains("unverified")),
        "{missing:?}"
    );

    // `/download`: off, and tokenless.
    let dl = ordo_api::download::router(
        state.clone(),
        None,
        ordo_api::download::DownloadState::new(
            Arc::new(NothingMarked),
            ordo_api::download::DownloadLimits::DEFAULT,
        ),
    );
    assert_eq!(get_download(&dl, Some(&t)).await, StatusCode::FORBIDDEN);
    assert_eq!(get_download(&dl, None).await, StatusCode::FORBIDDEN);
    let audit = access.take_audit();
    assert!(audit.iter().any(|a| a.contains("DownloadOff")), "{audit:?}");
    assert!(
        audit.iter().any(|a| a.contains("TokenMissing")),
        "{audit:?}"
    );
}
