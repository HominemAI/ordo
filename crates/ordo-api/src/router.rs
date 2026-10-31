//! The generated router: one path, no per-op code. The HTTP method is not a
//! client choice — it is the op's derived kind mirrored onto HTTP:
//! `GET /op/{name}` runs a one-shot read (`?live=1` upgrades it to an SSE
//! live subscription), `POST /op/{name}` runs a write, `DELETE /op/{name}`
//! runs a pure-delete. `GET /live` opens one stream that carries many
//! reads, added and dropped by `POST`/`DELETE /live/{stream}` (27s; see
//! [`crate::live`]). Pipeline order (design doc §1, with SH-19's space
//! step folded in): registry lookup → method enforcement → envelope split →
//! **space bind** → access check → param bind → refinement validation →
//! cursor decode → execute prepared plan **in the bound space**.
//!
//! The space does not stop at the door (SH2-1): it is handed to
//! [`crate::spec::OpExecutor::execute`] as its own argument, and the value
//! that gets there is checked against the value the access check was made
//! against ([`Gate::approved_space`]).
//!
//! Everything up to and including the access check is the **door**. A caller
//! who has not passed it learns as little as possible: an unknown op and a
//! forbidden op are one answer (see [`door_denial`]), and no pre-door response
//! carries the op's version headers, which would confirm existence by
//! themselves.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{any, delete, get, post, put};
use axum::Router;
use futures::stream::{self, StreamExt};
use serde_json::{Map, Value};
use tokio::sync::broadcast;

use crate::bind;
use crate::error::{ApiError, ErrorCode};
use crate::idempotency::{
    KeyBinding, ReplayCache, ReplayClaim, ReplayKey, RequestFingerprint, StoredResponse,
};
use crate::page::{self, PageCursor, PageKey};
use crate::refine::{self, Refinement};
use crate::registry::RegistryHandle;
use crate::spec::{
    AccessChecker, AccessDecision, DoorRefusal, ExecOutcome, ExecResult, InvalidationBus,
    OpExecutor, OpKind, OpSpec, PublicMode, QuotaDecision, QuotaGate, ReplayLease, TokenGate,
    ANONYMOUS,
};

const HEARTBEAT: Duration = Duration::from_secs(15);

/// How long a caller that lost the idempotency-key race waits for the winner's
/// outcome before giving up (Z-012). It must never fall through to execution:
/// executing after the wait is the double-write the reservation exists to
/// prevent, so the give-up answer is `idempotency_in_flight` (409) and the
/// client retries.
const REPLAY_WAIT_BUDGET: Duration = Duration::from_secs(5);

/// Poll interval while waiting on somebody else's reservation. The in-memory
/// cache has nothing to subscribe to; the Postgres-backed one uses
/// `LISTEN`/`NOTIFY` and wakes on the winner's commit instead of polling.
const REPLAY_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The header form of `$space` — preferred on `GET`/`DELETE`, where a query
/// string is the wrong place for something every access decision reads.
/// Public so a client can send the same spelling the door reads rather
/// than guessing at its case.
pub const SPACE_HEADER: &str = "x-ordo-space";

/// The header form of `$page`. Cursors are **data**, not addressing: a token
/// in a query string lands in proxy logs, browser history, and `Referer`
/// headers (SH-6), so generated SDKs prefer this. The query form stays
/// supported because a hand-written `curl` has to be able to page.
const PAGE_HEADER: &str = "x-ordo-page";
/// What a reconnecting device already holds, as the hash the server gave it
/// with the last completed load (`plan-replica-feed.md` C4d). A header rather
/// than a query value: it is data about the device, not addressing, and it
/// keeps out of proxy logs like the page cursor does.
const RESUME_HEADER: &str = "x-ordo-resume";

/// Longest space id the door will look at. A bound, not a schema: the id's
/// real shape is the platform's business (typed `spc_…` ids), and whether a
/// syntactically fine space *exists* is the access checker's answer, not a
/// distinct error code that would enumerate spaces.
const MAX_SPACE_LEN: usize = 128;

#[derive(Clone)]
pub struct ApiState {
    pub registry: Arc<RegistryHandle>,
    pub executor: Arc<dyn OpExecutor>,
    pub access: Arc<dyn AccessChecker>,
    /// 17n M2: checked in [`gate`], immediately after [`AccessChecker::check`]
    /// and never before — see [`QuotaGate`]. `Arc::new(NoQuota)` is the
    /// deliberate way to stand up a door with no quotas, the same posture
    /// `NoAttestation` gives layer-1 pinning.
    pub quota: Arc<dyn QuotaGate>,
    pub bus: Arc<dyn InvalidationBus>,
    pub replay: Arc<dyn ReplayCache>,
    /// Layer-1 client pinning: the signed head every response carries. See
    /// [`crate::attest`] — `Arc<NoAttestation>` is the deliberate way to
    /// stand up a door that cannot attest.
    pub attest: Arc<dyn crate::attest::AttestationSource>,
    /// Authenticates page tokens (SH-6). Deployment-wide; the token binds op
    /// and principal itself.
    pub page_key: PageKey,
    /// How `?live=1&sync=1` paces its reads. See [`crate::sync`].
    pub sync: crate::sync::SyncPacing,
    /// The shared live streams this door holds open (27s). See
    /// [`crate::live`]. `Default::default()` is an empty table.
    pub live_streams: crate::live::LiveStreams,
    /// How large one `$refine` may be (`ORDO_REFINE_MAX_CONDITIONS`,
    /// `ORDO_REFINE_MAX_IN_VALUES`). See [`crate::refine::RefineLimits`];
    /// `Default::default()` is the shipped defaults.
    pub refine_limits: crate::refine::RefineLimits,
    /// The public read path's cache and its CDN settings
    /// (`design-public-path.md` §3). `Default::default()` is the shipped
    /// settings with no purge hook. Reached only by a tokenless call to a
    /// public op; nothing else reads it.
    pub public: crate::public::PublicPath,
}

/// The op route's request-body ceiling.
///
/// **This number was already governing and nobody had written it down.**
/// `dispatch` takes `body: Bytes` — fully buffered — and there was no
/// `DefaultBodyLimit` anywhere in the tree, so axum's implicit 2 MB
/// default was the operative bound: untested, undocumented, and
/// indistinguishable from a deliberate choice. Stating it makes it a
/// decision, and pins it against the day somebody changes axum's default.
///
/// 2 MiB is generous for a JSON envelope of named parameters and far too
/// small for a file, which is the correct shape: bytes go through the blob
/// door, and an op body that approaches this number is a design mistake
/// this limit should surface rather than absorb.
pub const OP_BODY_LIMIT: usize = 2 * 1024 * 1024;

/// Build the router. There is nothing op-specific here — every op the
/// registry ever serves flows through the one dispatch handler.
///
/// Ops only. A deployment that serves blobs builds
/// [`router_with_blobs`] instead; a deployment that does not is not
/// obliged to construct a [`crate::blob::BlobPort`] it has nothing to
/// point at.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/op/{name}", any(dispatch))
        .route("/live", get(live_open))
        .route("/live/{stream}", post(live_add))
        .route("/live/{stream}/{key}", delete(live_drop))
        .layer(axum::extract::DefaultBodyLimit::max(OP_BODY_LIMIT))
        .with_state(state)
}

/// The op route plus the two blob routes.
///
/// The body limit is applied to the **op route only**. That is the point
/// of building the two as separate `Router`s and merging them: a single
/// `.layer()` over the whole tree would cap an upload at the envelope's
/// size, and a single limit raised to admit uploads would let a JSON
/// envelope be 100 MB. Two routes, two bounds, one merge.
pub fn router_with_blobs(state: ApiState, blobs: crate::blob::BlobState) -> Router {
    let ops = Router::new()
        .route("/op/{name}", any(dispatch))
        .route("/live", get(live_open))
        .route("/live/{stream}", post(live_add))
        .route("/live/{stream}/{key}", delete(live_drop))
        .layer(axum::extract::DefaultBodyLimit::max(OP_BODY_LIMIT))
        .with_state(state.clone());

    // `DefaultBodyLimit::disable()` on the blob routes, and it is not a
    // hole: the bound is enforced by `Content-Length` against
    // `BlobLimits::max_object` BEFORE the body is read (see
    // `crate::blob::length_required` for why the length must be declared
    // rather than discovered), and again by the store, which writes
    // exactly the declared number of bytes and refuses a body that
    // disagrees. axum's limit is a buffer ceiling for a handler that
    // buffers; these handlers stream.
    let blob = Router::new()
        .route("/blob/{entity}/{field}/{id}", put(blob_put))
        .route("/blob/{handle}", get(blob_get))
        .route("/blob/{handle}/meta", get(blob_meta))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state((state, blobs));

    ops.merge(blob)
}

// ── backend functions ───────────────────────────────────────────────────

/// A function's JSON input, at most (`plan-backend-functions.md` §7).
pub const FN_BODY_LIMIT: usize = 1024 * 1024;

/// `POST /fn/<name>`, merged into the door by a host that runs functions
/// (`crate::functions`). Its own `Router` for the same reason the blob
/// routes are: its body limit is its own.
pub fn function_routes(
    state: ApiState,
    functions: Arc<dyn crate::functions::FunctionRunner>,
) -> Router {
    Router::new()
        .route("/fn/{name}", any(fn_dispatch))
        .layer(axum::extract::DefaultBodyLimit::max(FN_BODY_LIMIT))
        .with_state((state, functions))
}

/// The op door's steps, in its order, for a function: the token gate, the
/// lookup that answers an unknown name as a forbidden one, the gate (method,
/// envelope, space, access, quota), typed parameters, and the idempotency
/// reservation. Then the runner. Its refusals are an [`ExecError`] and are
/// mapped by [`ApiError::from_exec`], as an op's are.
async fn fn_dispatch(
    State((state, functions)): State<(ApiState, Arc<dyn crate::functions::FunctionRunner>)>,
    Path(name): Path<String>,
    method: Method,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(refusal) = token_refusal(&state, &headers, &name).await {
        return refusal.into_response();
    }
    let Some(spec) = functions.spec(&name) else {
        let token = bearer(&headers);
        let why = match token {
            None => DoorRefusal::TokenMissing,
            Some(_) => DoorRefusal::UnregisteredOp,
        };
        return door_refusal(&state, &name, token, named_space(&headers), why).into_response();
    };
    let gate = match gate(&state, &spec, &method, &query, &headers, &body) {
        Ok(g) => g,
        Err(e) => return e.into_response(),
    };
    match run_function(&state, functions.as_ref(), &spec, &headers, gate).await {
        Ok((status, body)) => (status, Json(body)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn run_function(
    state: &ApiState,
    functions: &dyn crate::functions::FunctionRunner,
    spec: &OpSpec,
    headers: &HeaderMap,
    gate: Gate,
) -> Result<(StatusCode, Value), ApiError> {
    let space = gate.approved_space(spec)?.to_string();
    let keyed_as = gate.keyed_as();
    let bound = bind::bind_params(spec, &gate.params)?;
    let idem_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let slot = idem_key.clone().map(|key| {
        let replay_key = ReplayKey {
            space: space.clone(),
            principal: keyed_as.clone(),
            op: spec.name.clone(),
            key,
        };
        let fingerprint =
            RequestFingerprint::compute(&space, &spec.name, &spec.version_hash, &bound, None, None);
        (replay_key, fingerprint)
    });
    if let Some((replay_key, fingerprint)) = &slot {
        if let Some(outcome) = reserve_or_replay(
            state,
            spec,
            replay_key,
            fingerprint,
            KeyBinding::UntilExpiry,
        )
        .await?
        {
            return Ok((outcome.status, outcome.body));
        }
    }
    let call = crate::functions::FnInvocation {
        name: spec.name.clone(),
        space,
        principal: gate.principal.clone(),
        agent: gate.agent.clone(),
        args: bound.0,
        idempotency_key: idem_key,
    };
    match functions.call(call).await {
        Ok(result) => {
            let body = serde_json::json!({ "result": result });
            if let Some((replay_key, fingerprint)) = slot {
                state
                    .replay
                    .put(
                        replay_key,
                        fingerprint,
                        StoredResponse {
                            status: StatusCode::OK.as_u16(),
                            body: body.clone(),
                        },
                        KeyBinding::UntilExpiry,
                    )
                    .await;
            }
            Ok((StatusCode::OK, body))
        }
        Err(e) => {
            // A failed run is decided again on a retry: the key is freed.
            if let Some((replay_key, _)) = &slot {
                state.replay.abandon(replay_key).await;
            }
            Err(ApiError::from_exec(spec, e))
        }
    }
}

// ── the blob door ────────────────────────────────────────────────────────
//
// Two handlers, sharing the op door's access check, its error inventory
// and its denial shape. See `crate::blob` for the argument that these are
// a second ROUTE and not a second door, and for the authorization ruling:
// reading a blob is a read of its row, writing one is a write of its row.

/// A correlation for the key release, derived rather than accepted.
///
/// Every `KeyProvider::release` names the request it runs under, and a
/// provider that tracks correlations refuses one with no live job behind
/// it. A caller-supplied value would let the caller choose what their key
/// use is attributed to, which is the whole thing the correlation exists
/// to prevent.
fn blob_correlation(verb: crate::blob::BlobVerb, principal: &str, tag: &str) -> String {
    let v = match verb {
        crate::blob::BlobVerb::Read => "get",
        crate::blob::BlobVerb::Write => "put",
    };
    format!("blob.{v}:{principal}:{tag}")
}

/// The blob door's own space bind and access check.
///
/// Deliberately not `gate()`: that function's first act is method
/// enforcement against an `OpSpec`'s derived kind, and there is no
/// `OpSpec` here — the routes are addressed by entity and handle, not by
/// op name. What IS shared is everything that matters: the same header,
/// the same `AccessChecker`, the same `door_denial`, so an unknown entity
/// and a forbidden one answer identically.
async fn blob_gate(
    state: &ApiState,
    headers: &HeaderMap,
    verb: crate::blob::BlobVerb,
    entity_for_check: &str,
) -> Result<(String, String, Option<String>), ApiError> {
    let op = verb.std_op(entity_for_check);
    // The anonymous door for files (`design-public-path.md` §1, "Blob
    // reads"): a tokenless read runs as the anonymous principal in the
    // deployment's space, and `BlobPort::read` serves it only when the
    // handle's field is public. Files follow their row. A named space that is
    // not the deployment's is the door's one answer.
    if bearer(headers).is_none() && verb == crate::blob::BlobVerb::Read {
        let named = headers
            .get(SPACE_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::trim);
        return match state.access.anonymous_space() {
            Some(home) if named.is_none_or(|n| n == home) => {
                Ok((home, ANONYMOUS.to_string(), None))
            }
            _ => Err(door_refusal(
                state,
                &op,
                None,
                named,
                DoorRefusal::TokenMissing,
            )),
        };
    }
    let space = headers
        .get(SPACE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::MissingSpace,
                "every blob request names its space: send the X-Ordo-Space header",
            )
            .with_param("$space")
        })?
        .to_string();

    let Some(token) = bearer(headers) else {
        return Err(door_refusal(
            state,
            &op,
            None,
            Some(&space),
            DoorRefusal::TokenMissing,
        ));
    };
    // The token gate runs here as it does at `dispatch`, and first. A share
    // grant that is no longer live (a revoked link, a guest who left) must
    // close the file door as it closes the op door: the file of a row is the
    // row. Without this a revoked link would still serve every file it was
    // ever given.
    match state.access.token_gate(token).await {
        TokenGate::Pass => {}
        TokenGate::Busy => return Err(ApiError::server_busy().with_op(&op)),
        TokenGate::ShareNotLive => return Err(denied_after_check(&op, Some(token))),
        TokenGate::DelegationNotLive => {
            return Err(ApiError::new(
                ErrorCode::DelegationRequired,
                "this token acts on behalf of somebody, and that delegation is no longer \
                 live: it was withdrawn, has lapsed, or is unknown. A new one has to be \
                 consented to by the person being acted for.",
            ))
        }
    }
    match state.access.check(token, &space, &op) {
        AccessDecision::Allow { principal, agent } => Ok((space, principal, agent)),
        AccessDecision::Deny => match state.access.not_ready(token, &space) {
            Some(wait) => Err(ApiError::tenant_not_ready(&op, wait)),
            None if state.access.behind(token, &space) => Err(ApiError::server_busy().with_op(&op)),
            None if state.access.presence_required(token, &space, &op) => {
                Err(ApiError::presence_required(&op))
            }
            None => Err(denied_after_check(&op, Some(token))),
        },
    }
}

/// `PUT /blob/{entity}/{field}/{id}` — the bytes in.
async fn blob_put(
    State((state, blobs)): State<(ApiState, crate::blob::BlobState)>,
    Path((entity, field, row_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    use crate::blob::BlobVerb;

    let (space, principal, agent) =
        match blob_gate(&state, &headers, BlobVerb::Write, &entity).await {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };

    // The length is DECLARED, and refused against the cap before a byte is
    // read. A length discovered at the end is a cap enforced after the
    // bytes arrived.
    let Some(declared) = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return crate::blob::length_required().into_response();
    };
    if declared > blobs.limits.max_object {
        return crate::blob::too_large(blobs.limits.max_object, declared).into_response();
    }

    // A mime the caller supplies is a *label*, never a decision: nothing
    // downstream branches on it, it is sealed beside the bytes, and it is
    // handed back on a read for the browser's benefit. What it must not be
    // is a header injection, so it is bounded and filtered here rather
    // than trusted.
    let mime = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|m| {
            !m.is_empty() && m.len() <= 255 && m.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
        })
        .unwrap_or("application/octet-stream")
        .to_string();

    let req = crate::blob::BlobRequest {
        space,
        correlation: blob_correlation(BlobVerb::Write, &principal, &row_id),
        principal,
        agent,
        verb: BlobVerb::Write,
        entity: Some(entity),
        field: Some(field),
        row_id: Some(row_id),
        handle_hex: None,
    };

    let stream = body
        .into_data_stream()
        .map(|r| r.map(|b| b.to_vec()).map_err(|e| e.to_string()));

    match blobs
        .port
        .write(&req, declared, &mime, Box::pin(stream))
        .await
    {
        Ok(()) => (StatusCode::NO_CONTENT, ()).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /blob/{handle}/meta` — the file's media type and size, no bytes.
///
/// Gated and guarded exactly as the read is (`std.$blob.get`, then the row
/// guard inside the port), so a caller who may not read the file learns
/// nothing about it, and the answers for an unknown handle and a hidden row
/// are the same.
async fn blob_meta(
    State((state, blobs)): State<(ApiState, crate::blob::BlobState)>,
    Path(handle): Path<String>,
    headers: HeaderMap,
) -> Response {
    use crate::blob::BlobVerb;
    let (space, principal, agent) =
        match blob_gate(&state, &headers, BlobVerb::Read, crate::blob::BLOB_ENTITY).await {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
    let anonymous = principal == ANONYMOUS;
    let req = crate::blob::BlobRequest {
        space,
        correlation: blob_correlation(BlobVerb::Read, &principal, &handle),
        principal,
        agent,
        verb: BlobVerb::Read,
        entity: None,
        field: None,
        row_id: None,
        handle_hex: Some(handle),
    };
    match blobs.port.meta(&req).await {
        Err(e) => e.into_response(),
        Ok(m) => {
            let body = serde_json::json!({ "mime": m.mime, "size": m.size }).to_string();
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .header(
                    "cache-control",
                    if anonymous {
                        state.public.headers("std.$blob.get", "").cache_control
                    } else {
                        "private, max-age=31536000, immutable".to_string()
                    },
                )
                .body(axum::body::Body::from(body))
                .unwrap_or_else(|_| {
                    ApiError::new(ErrorCode::Internal, "blob response").into_response()
                })
        }
    }
}

/// `GET /blob/{handle}` — the bytes out.
async fn blob_get(
    State((state, blobs)): State<(ApiState, crate::blob::BlobState)>,
    Path(handle): Path<String>,
    headers: HeaderMap,
) -> Response {
    use crate::blob::BlobVerb;

    // The entity is not in the URL, so the access check cannot name a
    // per-entity op before the handle is resolved — and resolving it is
    // itself a read. So the door check runs against the reserved
    // pseudo-entity `$blob`, which no schema can declare (the lexer cannot
    // produce a `$`-prefixed identifier), and the REAL authorization is
    // the row guard inside `BlobPort::read`, which knows the entity
    // because `$blobref` stores it.
    //
    // That ordering is deliberate rather than convenient: checking a
    // per-entity op here would require resolving the handle to an entity
    // first, which turns the door into an oracle answering "does this
    // handle exist, and in which entity?" for any caller who can guess
    // sixteen bytes.
    let (space, principal, agent) =
        match blob_gate(&state, &headers, BlobVerb::Read, crate::blob::BLOB_ENTITY).await {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };

    let anonymous = principal == ANONYMOUS;
    let range = crate::blob::parse_range(headers.get("range").and_then(|v| v.to_str().ok()));
    let req = crate::blob::BlobRequest {
        space,
        correlation: blob_correlation(BlobVerb::Read, &principal, &handle),
        principal,
        agent,
        verb: BlobVerb::Read,
        entity: None,
        field: None,
        row_id: None,
        handle_hex: Some(handle),
    };

    let (from, len) = range.unwrap_or((0, None));
    match blobs.port.read(&req, from, len).await {
        // The port refuses a visitor a file whose field is not public with
        // the door's one answer; recorded here, where the door is.
        Err(e) if anonymous && e.code == ErrorCode::AccessDenied => recorded(
            &state,
            None,
            Some(&req.space),
            &BlobVerb::Read.std_op(crate::blob::BLOB_ENTITY),
            DoorRefusal::TokenMissing,
            e,
        )
        .into_response(),
        Err(e) => e.into_response(),
        Ok(read) => {
            let mut res = axum::response::Response::builder()
                .status(if read.partial {
                    StatusCode::PARTIAL_CONTENT
                } else {
                    StatusCode::OK
                })
                .header("content-length", read.bytes.len().to_string())
                // Ranges are servable, and saying so is what makes a
                // progressive player ask for one.
                .header("accept-ranges", "bytes")
                // A handle is never reused, so the bytes behind one can
                // never change — which is exactly what `immutable` means
                // and is the cacheability DL §2 claims. `private` because
                // the object passed a per-caller access check: a shared
                // cache holding it would serve it to the next caller
                // without one.
                .header(
                    "cache-control",
                    if anonymous {
                        // A public file is cached as a public result is:
                        // a shared cache may hold it for `s-maxage`, so an
                        // erased file leaves a CDN within that bound
                        // (`design-public-path.md` §3).
                        state.public.headers("std.$blob.get", "").cache_control
                    } else {
                        "private, max-age=31536000, immutable".to_string()
                    },
                );
            // The mime is a caller-supplied label: the type, disposition,
            // sandbox and `nosniff` are set in the one place stored bytes
            // are served from.
            res = crate::blob::stored_bytes(
                res,
                crate::blob::Stored::File {
                    mime: &read.mime,
                    // No per-field choice yet (v2): the default table.
                    shown: None,
                },
            );
            if read.partial {
                res = res.header(
                    "content-range",
                    format!(
                        "bytes {}-{}/{}",
                        read.from,
                        read.from + read.bytes.len() as u64 - 1,
                        read.plain_len
                    ),
                );
            }
            res.body(axum::body::Body::from(read.bytes))
                .unwrap_or_else(|_| {
                    ApiError::new(ErrorCode::Internal, "blob response").into_response()
                })
        }
    }
}

// ── method ↔ kind mirror ──

/// The op's kind as prose plus the one HTTP method that serves it.
///
/// **Public because it is the authority another document mirrors.** The
/// interface descriptor (`ordo_plan::interface`) states the method a
/// generated client should send, and it cannot call this — `ordo-api`
/// depends on no sibling crate by design, so the dependency cannot run the
/// other way either. Exporting the one function makes the mirror joinable:
/// `ordo-host`'s `tests/interface_surface.rs` walks the whole registry and
/// compares, exactly as `tests/cost_class.rs` does for the two spellings of
/// the cost model.
pub fn method_for(kind: OpKind) -> (&'static str, &'static str) {
    match kind {
        OpKind::Read => ("read", "GET"),
        OpKind::Write => ("write", "POST"),
        OpKind::Delete => ("delete", "DELETE"),
    }
}

fn method_mismatch(op: &OpSpec) -> ApiError {
    let (kind, method) = method_for(op.kind);
    ApiError::for_op(
        ErrorCode::MethodMismatch,
        op,
        format!("op `{}` is a {kind} op — call it with {method}", op.name),
    )
    .with_allow(method)
}

// ── dispatch: every method for /op/{name} lands here ──

async fn dispatch(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    method: Method,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // The token gate, FIRST — ahead of the op registry, and that ordering is
    // the whole reason `delegation_required` is allowed to be its own class.
    // A caller whose delegation has been withdrawn gets the identical answer
    // for a real op name, a forbidden one, and a guess, so the class
    // correlates with nothing about the deployment. Moving this below the
    // registry lookup would turn it into an op-existence oracle overnight.
    if let Some(refusal) = token_refusal(&state, &headers, &name).await {
        return refusal.into_response();
    }

    // The spec is cloned here, at dispatch — everything below is immune to a
    // concurrent registry swap.
    let Some(op) = state.registry.current().get(&name) else {
        // An op nobody registered answers exactly as a forbidden op does.
        let token = bearer(&headers);
        if token.is_none() {
            state.public.count("-", "door", "refused");
        }
        let why = match token {
            None => DoorRefusal::TokenMissing,
            Some(_) => DoorRefusal::UnregisteredOp,
        };
        return door_refusal(&state, &name, token, named_space(&headers), why).into_response();
    };

    // Stage 1 — the door. Answered bare: no version headers, because
    // `x-ordo-op-version` on a rejection would confirm the op exists and leak
    // its deployed hash to a caller who never got in.
    let gate = match gate(&state, &op, &method, &query, &headers, &body) {
        Ok(gate) => gate,
        Err(err) => {
            // Every tokenless request is counted, the refused ones too. A
            // refused one is counted under `-`, never its name, so the
            // count says nothing a probe could not already see.
            if bearer(&headers).is_none() {
                match op.public {
                    PublicMode::None => state.public.count("-", "door", "refused"),
                    PublicMode::Read => state.public.count(&op.name, "read", "refused"),
                    PublicMode::Write | PublicMode::WriteKeyed => {
                        state.public.count(&op.name, "write", "refused")
                    }
                }
            }
            return err.into_response();
        }
    };

    // Still the door: a person the app developer requires a verified email
    // address of, calling an op it names, without one (owner ruling
    // 2026-10-22). Asked only after the access check allowed the call, so
    // it says nothing to a caller who may not call the op.
    if gate.principal != ANONYMOUS {
        if let Some(token) = bearer(&headers) {
            if let Some(held) = state
                .access
                .email_required(token, &gate.principal, &op.name)
                .await
            {
                return if held.retryable {
                    ApiError::email_unreadable(&op.name, &held.message)
                } else {
                    ApiError::email_required(&op.name, &held.message)
                }
                .into_response();
            }
        }
    }

    // Stage 2 — past the door. The op is real to this caller now, so ordinary
    // typed errors and the version headers are safe to speak.
    //
    // A visitor with no token reached a public read: the public path, which
    // answers from the cache and skips what a member's read pays for
    // (`design-public-path.md` §2).
    if gate.principal == ANONYMOUS {
        let response = if op.public.is_write() {
            public_write(&state, &op, &headers, gate)
                .await
                .unwrap_or_else(IntoResponse::into_response)
        } else if wants_live(&query) {
            state.public.count(&op.name, "read", "refused");
            ApiError::for_op(
                ErrorCode::NotLive,
                &op,
                "live updates on a public read are not built yet for a caller with no \
                 token; read it once, or sign in",
            )
            .into_response()
        } else {
            let answer = public_call(&state, &op, &headers, gate).await;
            let outcome = match &answer {
                Ok(r) => match r.headers().get("x-ordo-cache").map(|v| v.as_bytes()) {
                    Some(b"hit") => "hit",
                    _ => "miss",
                },
                Err(e) if e.code == ErrorCode::QuotaExceeded => "limited",
                Err(_) => "refused",
            };
            state.public.count(&op.name, "read", outcome);
            answer.unwrap_or_else(IntoResponse::into_response)
        };
        // Marked for the CORS layer: a public answer carries no credential,
        // so any page may read it (`design-public-path.md` §8).
        let mut response = response;
        response.extensions_mut().insert(crate::cors::PublicAnswer);
        return with_version_headers(response, &op, &headers);
    }
    let response = if method == Method::GET && wants_live(&query) {
        let resume = headers
            .get(RESUME_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        live_inner(&state, &op, gate, &headers, wants_sync(&query), resume)
            .await
            .unwrap_or_else(IntoResponse::into_response)
    } else {
        // The pin is read for the space this caller was ADMITTED to, and only
        // past the door — for the same reason the version headers are. A
        // position on a door rejection would tell a caller who never got in
        // that the space exists and how much history it has.
        let space = gate.space.clone();
        match run_call(&state, &op, &headers, gate).await {
            Ok(outcome) => {
                // A read that reports the position its rows were read at is
                // pinned there, so the signed head is the rows' version and
                // not a later one (C1). Anything else pins the current head.
                let pin = match outcome.position {
                    Some(upto) => state.attest.head_at(&space, upto).await,
                    None => state.attest.head(&space).await,
                };
                call_response(outcome, pin)
            }
            Err(err) => err.into_response(),
        }
    };
    with_version_headers(kept_off_disk(response), &op, &headers)
}

/// A member's answer is never stored by the browser's HTTP cache.
///
/// Two reasons. The rows are the caller's own, and a cache on disk would hold
/// them in the clear beside the device copy that seals them. And a stream
/// that a cache is writing holds the cache entry for its URL: Firefox makes a
/// second request for the same URL wait on that entry until the first
/// response ends, which for a live feed is never. The space rides in a
/// header, so two feeds of one op in two spaces, or in two tabs, share a URL
/// (notes e2e, 2026-10-20: the second tab's feed stayed pending). `no-store`
/// keeps every answer out of the cache, so no entry is held.
///
/// An answer that already says how it may be cached keeps what it says,
/// except a stream, which is always `no-store`.
fn kept_off_disk(mut response: Response) -> Response {
    let stream = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream"));
    let headers = response.headers_mut();
    if stream || !headers.contains_key(axum::http::header::CACHE_CONTROL) {
        headers.insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        );
    }
    response
}

/// The token gate's answer, when it refuses (see [`dispatch`] for why it
/// runs first). Shared by the op route and the shared live stream's `POST`,
/// so a read added to a stream meets the same gate as one called alone.
pub(crate) async fn token_refusal(
    state: &ApiState,
    headers: &HeaderMap,
    name: &str,
) -> Option<ApiError> {
    let token = bearer(headers)?;
    match state.access.token_gate(token).await {
        TokenGate::Busy => Some(ApiError::server_busy().with_op(name)),
        TokenGate::ShareNotLive => Some(denied_after_check(name, Some(token))),
        TokenGate::DelegationNotLive => Some(ApiError::new(
            ErrorCode::DelegationRequired,
            "this token acts on behalf of somebody, and that delegation is no longer \
             live — it was withdrawn, has lapsed, or is unknown. A new one has to be \
             consented to by the person being acted for.",
        )),
        _ => None,
    }
}

/// The door a long-lived read stands behind, asked again for as long as the
/// read is open.
///
/// A feed (`?live=1`, `?live=1&sync=1`, a read on `/live`) is admitted once,
/// but what admitted it can end while it runs: a delegation is withdrawn, a
/// share is revoked, the person logs out or is removed. So every long-lived
/// read asks this door before every pass and on an idle tick
/// ([`crate::LiveSettings::recheck`]), and ends with an `error` frame when it
/// refuses. It runs both halves of the one-shot door: the token gate
/// ([`token_refusal`]: delegation and share liveness) and [`check_access`]
/// (membership, session revocation). No long-lived path calls either half on
/// its own, so none can forget the other.
///
/// The cost of one pass: one token verify (an HMAC), one indexed lookup in
/// the delegation or share registry for a token that claims one (nothing for
/// a plain session), and the in-memory access check.
#[derive(Clone)]
pub(crate) struct Door {
    state: ApiState,
    headers: HeaderMap,
}

impl Door {
    pub(crate) fn new(state: &ApiState, headers: &HeaderMap) -> Door {
        // Only the credential decides the door's answer, so only it is kept.
        let mut kept = HeaderMap::new();
        if let Some(v) = headers.get(axum::http::header::AUTHORIZATION) {
            kept.insert(axum::http::header::AUTHORIZATION, v.clone());
        }
        Door {
            state: state.clone(),
            headers: kept,
        }
    }

    /// The token gate alone: what holds for the token whatever it reads.
    /// `/live` asks it once per tick for the whole stream.
    pub(crate) async fn token(&self, name: &str) -> Result<(), ApiError> {
        match token_refusal(&self.state, &self.headers, name).await {
            Some(refusal) => Err(refusal),
            None => Ok(()),
        }
    }

    /// The access check alone, for one read, after [`Door::token`] passed.
    pub(crate) fn access(&self, op: &OpSpec, space: &str) -> Result<(), ApiError> {
        check_access(&self.state, op, space, &self.headers).map(|_| ())
    }

    /// The whole door for one read: the token gate, then the access check.
    pub(crate) async fn admit(&self, op: &OpSpec, space: &str) -> Result<(), ApiError> {
        self.token(&op.name).await?;
        self.access(op, space)
    }

    /// How often a quiet feed asks the door anyway.
    pub(crate) fn recheck(&self) -> Duration {
        self.state.live_streams.settings().recheck
    }
}

/// The frame a feed ends with when the door refuses it.
pub(crate) fn refusal_frame(e: &ApiError, key: Option<&str>) -> Event {
    let mut body = e.envelope();
    if let (Some(key), Some(obj)) = (key, body.as_object_mut()) {
        obj.insert("key".into(), Value::String(key.to_string()));
    }
    Event::default().event("error").data(body.to_string())
}

/// A ticker for [`Door::recheck`] whose first tick is one period away, not
/// now: the feed was admitted a moment ago.
pub(crate) fn recheck_ticker(period: Duration) -> tokio::time::Interval {
    let mut t = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    t
}

fn wants_live(query: &HashMap<String, String>) -> bool {
    query.get("live").is_some_and(|v| v == "1" || v == "true")
}

/// `sync` is read only beside `live`: a sync is a mode of the live
/// subscription (`plan-replica-feed.md` §2), and without `live` the key is
/// an ordinary parameter name like any other.
fn wants_sync(query: &HashMap<String, String>) -> bool {
    wants_live(query) && query.get("sync").is_some_and(|v| v == "1" || v == "true")
}

struct CallOutcome {
    status: StatusCode,
    body: Value,
    replayed: bool,
    /// The command-log position a read's rows were read at, when the
    /// executor reported one. See [`ExecResult::position`].
    position: Option<u64>,
}

/// Layer-1 pinning headers. Four rather than one packed blob: a client that
/// only wants to notice movement reads the position, and one that verifies
/// needs the head, the key id and the signature separately anyway — packing
/// them would mean publishing an encoding whose only consumer would have to
/// unpack it again.
const POSITION_HEADER: &str = "x-ordo-position";
const HEAD_HEADER: &str = "x-ordo-head";
const HEAD_KEY_HEADER: &str = "x-ordo-head-key";
const HEAD_SIG_HEADER: &str = "x-ordo-head-sig";

/// The floor a prune left behind: where recomputation still starts.
///
/// A fifth header rather than a field folded into one of the four, because
/// it is a different **kind** of claim: the other four are signed and this
/// one is not ([`crate::attest::HeadPin::verifiable_from`]). Packing an
/// unsigned value into `x-ordo-head` would invite a client to treat the
/// signature as covering it.
const VERIFIABLE_FROM_HEADER: &str = "x-ordo-verifiable-from";

fn call_response(outcome: CallOutcome, pin: Option<crate::attest::HeadPin>) -> Response {
    let mut resp = (outcome.status, Json(outcome.body)).into_response();
    if outcome.replayed {
        resp.headers_mut()
            .insert("x-ordo-idempotent-replay", HeaderValue::from_static("true"));
    }
    if let Some(pin) = pin {
        let h = resp.headers_mut();
        // A header that will not encode is dropped rather than failing the
        // request: pinning is an auditing convenience, and an unattested
        // answer beats no answer. A client that requires a pin sees its
        // absence, which is the signal it needs anyway.
        for (name, value) in [
            (POSITION_HEADER, pin.upto.to_string()),
            (HEAD_HEADER, pin.head),
            (HEAD_KEY_HEADER, pin.key_id),
            (HEAD_SIG_HEADER, pin.signature),
            (VERIFIABLE_FROM_HEADER, pin.verifiable_from.to_string()),
        ] {
            if let Ok(v) = HeaderValue::from_str(&value) {
                h.insert(name, v);
            }
        }
    }
    resp
}

// ── stage 1: the door ──

/// What the door produces for the rest of the pipeline: the bound space, the
/// principal the access check resolved, and the raw request envelope.
struct Gate {
    space: String,
    /// The exact string [`check_access`] handed the [`AccessChecker`], copied
    /// back out of the call. It exists so the space that reaches the executor
    /// can be compared against the space that was *authorized*, rather than
    /// assumed equal because they were once the same variable
    /// ([`Gate::approved_space`]).
    checked_space: String,
    principal: String,
    /// The principal presenting the token when it acts for `principal`
    /// under a delegation ([`AccessDecision::Allow`]). Never a guard input.
    agent: Option<String>,
    params: Map<String, Value>,
    refine_raw: Option<Value>,
    page_raw: Option<Value>,
}

impl Gate {
    /// The space the executor may plan against: the one the door bound *and*
    /// the one the access check was made against, verified equal here (SH2-1
    /// / Y-015).
    ///
    /// A disagreement is impossible through this file and is still checked,
    /// because the failure it would cause — approve one space, execute
    /// against another — is the exact break `$space` exists to prevent. It
    /// answers `internal`, loudly: there is no defensible default space to
    /// fall back to, so no request proceeds.
    fn approved_space(&self, op: &OpSpec) -> Result<&str, ApiError> {
        if self.space != self.checked_space {
            return Err(ApiError::for_op(
                ErrorCode::Internal,
                op,
                "the space the door authorized and the space bound for execution disagree",
            ));
        }
        Ok(&self.space)
    }

    /// Hand the door's principal to the executor: `$caller` for the row
    /// guards, and [`AGENT_PARAM`] when a delegated token is acting for
    /// that caller, so the command log can name both.
    ///
    /// Both are reserved `$`-keys no declared parameter can shadow (SH2-1),
    /// taken from the access decision like every other attribution
    /// (SH-17). A client cannot send either: `$caller` is `caller_supplied`
    /// and every other `$`-key is `unknown_envelope_key`.
    fn bind_caller(&self, bound: &mut crate::BoundParams) {
        bound.0.insert(
            "$caller".into(),
            serde_json::Value::String(self.principal.clone()),
        );
        if let Some(agent) = &self.agent {
            bound
                .0
                .insert(AGENT_PARAM.into(), serde_json::Value::String(agent.clone()));
        }
    }

    /// Whose replay cache and page cursors this call uses: the principal,
    /// or under a delegation the delegator **and** the agent together.
    ///
    /// A person and each agent acting for them get separate namespaces
    /// (owner's lead, 2026-10-08). Keyed on the delegator alone, an agent
    /// could replay a response the person stored under the same
    /// `Idempotency-Key`, or carry on a page the person started, and two
    /// agents for one person could do the same to each other. The row
    /// guards still answer for the delegator; only these two keys widen.
    fn keyed_as(&self) -> String {
        match &self.agent {
            None => self.principal.clone(),
            Some(agent) => delegated_key(&self.principal, agent),
        }
    }
}

/// The replay and cursor key of a delegated call: `<delegator>+agent:<agent>`.
/// A plain principal never contains `+agent:`, so the two cannot collide.
fn delegated_key(principal: &str, agent: &str) -> String {
    format!("{principal}+agent:{agent}")
}

/// The reserved key under which a delegated call's **agent** reaches the
/// executor, beside `$caller` (the delegator). Read only where a command
/// group's actor is built; no guard or op body can name it.
pub const AGENT_PARAM: &str = "$agent";

/// Method enforcement → envelope split → space bind → access check.
///
/// Order note: the space is bound *before* the access check because the check
/// takes it (SH-19); the envelope split has to come first because that is
/// where `$space` lives on a `POST`. Nothing here touches the executor, and
/// binding/planning happen only after this returns.
fn gate(
    state: &ApiState,
    op: &OpSpec,
    method: &Method,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Gate, ApiError> {
    // Method enforcement first — a caller cannot even *address* a write as if
    // it were a read.
    let (_, expected) = method_for(op.kind);
    if method.as_str() != expected {
        return Err(method_mismatch(op));
    }

    let mut envelope = if method == Method::POST {
        split_envelope(op, body)?
    } else {
        split_query(op, query)?
    };
    envelope.page_raw = bind_page_token(op, headers, envelope.page_raw)?;
    // A visitor's call to a public op names no space: it runs in the
    // deployment's own (`design-public-path.md` §1). Named anyway, it must be
    // that one, and [`check_access`] refuses any other with the door's one
    // answer.
    let anonymous_home = (bearer(headers).is_none() && op.public != PublicMode::None)
        .then(|| state.access.anonymous_space())
        .flatten();
    let unnamed = envelope.space_raw.is_none() && !headers.contains_key(SPACE_HEADER);
    // No token and no space, and not the anonymous door: the one answer the
    // door gives an unknown op, before the space is bound. `missing_space`
    // here would tell a tokenless caller which op names are real.
    if unnamed && bearer(headers).is_none() && anonymous_home.is_none() {
        return Err(door_refusal(
            state,
            &op.name,
            None,
            None,
            DoorRefusal::TokenMissing,
        ));
    }
    let space = match anonymous_home {
        Some(home) if unnamed => home,
        _ => bind_space(op, headers, envelope.space_raw)?,
    };
    let (principal, agent, checked_space) = check_access(state, op, &space, headers)?;
    // 17n M2: the quota gate, **after** access and never before — a quota
    // refusal handed to a caller with no membership in the space would
    // disclose that the space exists and is busy, which is exactly what
    // `access_denied`'s uniform answer exists to prevent
    // (`corpus-rulings.md`, `write-quota`, 2026-09-19).
    //
    // Not for the anonymous principal: a public read is in the deployment's
    // space, not a tenant's, and its load is bounded by the cache and by
    // `ORDO_PUBLIC_MISSES_PER_MINUTE` on its misses (`design-public-path.md`
    // §2; audit 2026-10-11, finding 27).
    if principal != ANONYMOUS {
        quota_gate(state, op, &checked_space)?;
    }
    Ok(Gate {
        space,
        checked_space,
        principal,
        agent,
        params: envelope.params,
        refine_raw: envelope.refine_raw,
        page_raw: envelope.page_raw,
    })
}

/// The single answer the door ever gives, for every reason it could refuse
/// (C7 / X-038).
///
/// `unknown_op` (404) next to `access_denied` (403) lets any token enumerate
/// which ops exist **deployment-wide**: ask for a name, read the status. So an
/// unregistered name and a registered-but-forbidden op produce the identical
/// response — same status, same code, same message, same (absent) headers. The
/// answer is 403 rather than 404 because for a name nobody registered the
/// platform cannot know whether this caller would have been allowed, and the
/// safe assumption is that they would not.
///
/// Both cases are `access_denied`, the **door's** word. Row-level guard
/// rejection is `access_guard` and is raised only by the executor, after the
/// door opened; the two are never interchanged.
///
/// Residual channels, stated rather than assumed: `method_mismatch` (405) is
/// answered *before* the access check, so a caller who sends the wrong method
/// can distinguish a registered op from an unregistered one. `missing_space`
/// (400) is answered before it only to a caller who holds a token: with no
/// token and no space the door answers first, as it does for an unknown op.
/// Closing the method channel would mean checking access before method
/// enforcement, which the pipeline order pins the other way. It is a noisy
/// probe that the operational log's denial-volume alerting is meant to catch.
fn door_denial(op_name: &str, token: Option<&str>) -> ApiError {
    let message = match token {
        None => "missing bearer token",
        Some(_) => "op is not callable with this token",
    };
    ApiError::new(ErrorCode::AccessDenied, message).with_op(op_name)
}

/// The door's one answer for a denial that was **already recorded**: by
/// [`AccessChecker::check`] (a `Deny`) or by the token gate (a closed share
/// grant). Any other refusal goes through [`door_refusal`], which records
/// it. `tests/door_refusal_scan.rs` holds every caller to one of the two.
pub(crate) fn denied_after_check(op_name: &str, token: Option<&str>) -> ApiError {
    door_denial(op_name, token)
}

/// The one recording path for a door refusal `check` never saw: the record
/// first ([`AccessChecker::record_refusal`], to the space's stream when the
/// token has a relationship there, else the deployment's), then the
/// door's uniform answer.
pub(crate) fn door_refusal(
    state: &ApiState,
    op_name: &str,
    token: Option<&str>,
    space: Option<&str>,
    why: DoorRefusal,
) -> ApiError {
    recorded(
        state,
        token,
        space,
        op_name,
        why,
        door_denial(op_name, token),
    )
}

/// [`door_refusal`] for a refusal with its own answer (the download's
/// sentences, `/live`'s bare one): recorded the same way, answered `answer`.
pub(crate) fn recorded(
    state: &ApiState,
    token: Option<&str>,
    space: Option<&str>,
    op_name: &str,
    why: DoorRefusal,
    answer: ApiError,
) -> ApiError {
    state.access.record_refusal(token, space, op_name, why);
    answer
}

/// What the security record calls `GET /live` and its sub-routes, which
/// name no op.
pub(crate) const LIVE_GATE: &str = "live-gate";

/// The space the request names in its header, for a record. Not bound:
/// a refusal records what the caller sent.
pub(crate) fn named_space(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(SPACE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Bind the request's space (SH-19 / X-040).
///
/// Space is a first-class part of every request: `$space` joins `$refine` and
/// `$page` in the closed envelope key set on every transport, and
/// `X-Ordo-Space` is the header form that `GET`/`DELETE` prefer. It is
/// required — there is no "the caller's space", because a human belongs to
/// many and a token that pinned one would make the multi-space product
/// impossible.
///
/// Missing, empty, ill-formed, or contradictory (header and envelope
/// disagreeing) is `missing_space` (400) — a typed rejection, never a default.
/// Whether the space *exists*, and whether this principal is in it, is the
/// access checker's answer: a distinct "no such space" would enumerate the
/// deployment's spaces.
fn bind_space(
    op: &OpSpec,
    headers: &HeaderMap,
    envelope: Option<Value>,
) -> Result<String, ApiError> {
    let missing = |message: &str| {
        Err(ApiError::for_op(ErrorCode::MissingSpace, op, message.to_string()).with_param("$space"))
    };

    let header = headers
        .get(SPACE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    let from_envelope = match &envelope {
        None => None,
        Some(Value::String(s)) => Some(s.trim()),
        Some(_) => return missing("$space must be a string space id"),
    };

    let space = match (header, from_envelope) {
        (Some(h), Some(e)) if h != e => {
            return missing("X-Ordo-Space and $space name different spaces");
        }
        (Some(h), _) => h,
        (None, Some(e)) => e,
        (None, None) => {
            return missing(
                "every op call names its space: `$space` in the request envelope, \
                 or the X-Ordo-Space header",
            );
        }
    };

    if space.is_empty() || space.len() > MAX_SPACE_LEN {
        return missing("$space must be a non-empty space id");
    }
    if !well_formed_space(space) {
        return missing("$space is not a well-formed space id");
    }
    Ok(space.to_string())
}

/// Whether a string is a space id this door will bind.
///
/// Public because provisioning is the other end of the same rule: a space
/// created under a name the door cannot bind is a tenant with keys, rows
/// and a chain that no request can ever reach, and the two ends must not
/// read the rule twice and disagree about it. [`bind_space`] keeps its own
/// emptiness and length test above so that a caller gets the more specific
/// of the two messages; this is the whole rule in one answer.
pub fn well_formed_space(space: &str) -> bool {
    !space.is_empty()
        && space.len() <= MAX_SPACE_LEN
        && space
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Resolve the page token across its two carriers. `X-Ordo-Page` and `$page`
/// naming different tokens is `invalid_cursor` — the same reasoning as
/// `missing_space`: a request that says two things has said nothing, and
/// picking one would let whichever channel wins override the other.
fn bind_page_token(
    op: &OpSpec,
    headers: &HeaderMap,
    envelope: Option<Value>,
) -> Result<Option<Value>, ApiError> {
    let header = headers
        .get(PAGE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    match (header, envelope) {
        (None, envelope) => Ok(envelope),
        (Some(h), None) => Ok(Some(Value::String(h.to_string()))),
        (Some(h), Some(e)) if e.as_str() == Some(h) => Ok(Some(e)),
        (Some(_), Some(_)) => Err(ApiError::for_op(
            ErrorCode::InvalidCursor,
            op,
            "X-Ordo-Page and $page carry different tokens",
        )
        .with_param("$page")),
    }
}

/// The one check at the door. Returns the principal the checker resolved —
/// the idempotency cache keys on it (SH-17), and the token is interpreted
/// nowhere else — together with the space the decision was made **against**,
/// which the rest of the pipeline carries so the executor's space can be
/// checked against the authorized one rather than assumed.
pub(crate) fn check_access(
    state: &ApiState,
    op: &OpSpec,
    space: &str,
    headers: &HeaderMap,
) -> Result<(String, Option<String>, String), ApiError> {
    let token = bearer(headers);
    match token {
        // The anonymous door (`design-public-path.md` §1): a tokenless call
        // to a public read in the **active** artifact, in the deployment's
        // space, runs as the reserved anonymous principal. Anything else,
        // including a public op asked in another space, is the door's one
        // answer, byte for byte, so a probe learns nothing about which ops
        // or spaces exist.
        //
        // A public write is the same door: the compiler proved it only
        // creates rows, and the kind is checked again here because the door
        // trusts this field.
        None if (op.public == PublicMode::Read && op.kind == OpKind::Read)
            || (op.public.is_write() && op.kind == OpKind::Write) =>
        {
            match state.access.anonymous_space() {
                Some(home) if home == space => Ok((ANONYMOUS.to_string(), None, home)),
                _ => Err(door_refusal(
                    state,
                    &op.name,
                    None,
                    Some(space),
                    DoorRefusal::TokenMissing,
                )),
            }
        }
        None => Err(door_refusal(
            state,
            &op.name,
            None,
            Some(space),
            DoorRefusal::TokenMissing,
        )),
        Some(token) => match state.access.check(token, space, &op.name) {
            AccessDecision::Allow { principal, agent } => Ok((principal, agent, space.to_string())),
            AccessDecision::Deny => match state.access.not_ready(token, space) {
                // The person's own tenant, still being set up: a retryable
                // refusal rather than the uniform denial.
                Some(wait) => Err(ApiError::tenant_not_ready(&op.name, wait)),
                // The door's member list for this tenant is behind its
                // database: a retryable 503, never a denial it cannot back.
                None if state.access.behind(token, space) => {
                    Err(ApiError::server_busy().with_op(&op.name))
                }
                None if state.access.presence_required(token, space, &op.name) => {
                    Err(ApiError::presence_required(&op.name))
                }
                None => Err(denied_after_check(&op.name, Some(token))),
            },
        },
    }
}

/// The quota check (17n M2). Called from [`gate`] only, after
/// [`check_access`] has already named a principal — this function never
/// runs for a caller the access check refused.
///
/// `Within` and `Warn` both let the call through: a warning is a signal for
/// the deployment surface, not a refusal, and recording it is
/// [`QuotaGate::check`]'s own job, not this function's. Only `Exceeded`
/// stops the request, with [`ErrorCode::QuotaExceeded`] (429) naming the
/// quantity and nothing else — no other space, no raw counters that would
/// let a caller infer another tenant's traffic.
fn quota_gate(state: &ApiState, op: &OpSpec, space: &str) -> Result<(), ApiError> {
    match state.quota.check(space, op) {
        QuotaDecision::Within | QuotaDecision::Warn { .. } => Ok(()),
        QuotaDecision::Exceeded { quantity, .. } => Err(ApiError::for_op(
            ErrorCode::QuotaExceeded,
            op,
            format!(
                "space is over its quota for {quantity}. Raise the limit with `ordo quota \
                 set --space <space> --quantity {quantity} --limit <n>`, or wait for the \
                 period to roll over"
            ),
        )),
    }
}

// ── stage 2, public: bind → cache → execute ──

/// A public read by a visitor with no token (`design-public-path.md` §2, §3).
///
/// Declared parameters only: `$refine` is refused, because each refinement
/// would be a cache key of its own and an open one turns every request into
/// a database read (ruled, `public-path-design` ask 1). `$page` is allowed:
/// every visitor shares the anonymous principal, which is safe because every
/// visitor sees the same rows.
///
/// No audit row is written, no guard is walked and no membership is read:
/// the compiler proved the op reads public fields only (ruled, `public-data`
/// ask 10). The answer carries the CDN headers, and `If-None-Match` is
/// answered `304` from the stored `ETag`.
async fn public_call(
    state: &ApiState,
    op: &OpSpec,
    headers: &HeaderMap,
    mut gate: Gate,
) -> Result<Response, ApiError> {
    let space = gate.approved_space(op)?.to_string();
    if gate.refine_raw.take().is_some() {
        return Err(ApiError::for_op(
            ErrorCode::InvalidRefinement,
            op,
            "a public read takes its declared parameters only; `$refine` is refused on it \
             (each refinement would be a cache entry of its own)",
        )
        .with_param("$refine"));
    }
    let page_token = gate
        .page_raw
        .as_ref()
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let cursor = parse_cursor(state, &space, op, ANONYMOUS, gate.page_raw.take())?;
    let bound = bind::bind_params(op, &gate.params)?;

    let public = &state.public;
    let key =
        crate::public::PublicPath::key(&op.version_hash, &op.name, &bound.0, page_token.as_deref());
    // Subscribed before the first fetch, so a write that commits while the
    // fetch runs is heard and the answer it makes stale is not stored.
    public.ensure_subscribed(&state.bus, &op.name, &space).await;
    let if_none_match = headers.get("if-none-match").and_then(|v| v.to_str().ok());

    let (body, etag, outcome) = match public.lookup(&key) {
        Some(hit) => (hit.body, hit.etag, "hit"),
        None => {
            if !public.admit_miss(&op.name) {
                return Err(ApiError::for_op(
                    ErrorCode::QuotaExceeded,
                    op,
                    format!(
                        "this public read has sent {} cache misses to the database this \
                         minute, its limit; try again within a minute. The app developer \
                         can raise ORDO_PUBLIC_MISSES_PER_MINUTE, or set it to 0 to turn the \
                         cap off",
                        public.settings().misses_per_minute
                    ),
                ));
            }
            let ticket = public.ticket(key, &op.name);
            let mut bound = bound;
            gate.bind_caller(&mut bound);
            let result = match state
                .executor
                .execute(op, &space, bound, None, cursor, None)
                .await
            {
                Ok(ExecOutcome::Ran(result)) => result,
                Ok(ExecOutcome::AlreadyApplied) => {
                    return Err(ApiError::for_op(
                        ErrorCode::Internal,
                        op,
                        "a public read answered as an applied write",
                    ))
                }
                Err(e) => return Err(ApiError::from_exec(op, e)),
            };
            let envelope = success_envelope(state, &space, op, ANONYMOUS, result);
            let bytes = serde_json::to_vec(&envelope).unwrap_or_default();
            let (body, etag) = public.store(ticket, bytes);
            (body, etag, "miss")
        }
    };

    let described = public.headers(&op.name, &etag);
    let not_modified = crate::public::not_modified(if_none_match, &etag);
    let mut res = Response::builder()
        .status(if not_modified {
            StatusCode::NOT_MODIFIED
        } else {
            StatusCode::OK
        })
        .header("cache-control", described.cache_control)
        .header("etag", described.etag)
        .header("cache-tag", described.tags.join(", "))
        .header("surrogate-key", described.tags.join(" "))
        .header("vary", "Accept")
        .header("x-ordo-cache", outcome);
    if !not_modified {
        res = res.header("content-type", "application/json");
    }
    let body = if not_modified {
        axum::body::Body::empty()
    } else {
        axum::body::Body::from(body.as_ref().clone())
    };
    res.body(body)
        .map_err(|_| ApiError::for_op(ErrorCode::Internal, op, "public response"))
}

/// The longest human-check token read. Turnstile documents 2048 characters;
/// a longer header is not a token and is refused without a provider call.
const HUMAN_TOKEN_MAX: usize = 4096;

/// A public write by a visitor with no token (`design-public-path.md` §5).
///
/// The compiler proved the op only creates rows, in unguarded entities, and
/// that no ref it sets from a visitor's value can probe a private row
/// (`ordo_plan::public`). What runs here, in order:
///
/// 1. **The hourly cap is read** (`ORDO_PUBLIC_WRITES_PER_HOUR`). A full hour
///    is refused before the provider is asked, so a flood costs no calls.
/// 2. **The human check** (`ORDO_PUBLIC_HUMAN_CHECK`), on by default. The
///    provider confirms the `X-Ordo-Human` token. No provider, no token, a
///    refused token or a provider that cannot be asked all refuse the write.
/// 3. **The cap is taken.** Only a write that passed the check uses the
///    hour, so a bot's bad tokens cannot lock real visitors out.
/// 4. **The ordinary write pipeline**, unchanged: bind, idempotency,
///    execute, command log, and the invalidation that drops public answers
///    reading the entity. The actor is `$anonymous`. No address is recorded.
///
/// The answer is never cached, carries no signed head (that would show a
/// visitor the deployment space's history length), and names each created
/// row's `id` and nothing else. A create keyed on a unique field names no id
/// at all and answers the same whether the row was new or already there
/// ([`keyed_answer`]).
async fn public_write(
    state: &ApiState,
    op: &OpSpec,
    headers: &HeaderMap,
    gate: Gate,
) -> Result<Response, ApiError> {
    let public = &state.public;
    let settings = public.settings().clone();
    let count = |outcome| public.count(&op.name, "write", outcome);
    let over_cap = || {
        count("limited");
        ApiError::for_op(
            ErrorCode::QuotaExceeded,
            op,
            format!(
                "this public write has made {} writes this hour, its limit; try again later. \
                 The app developer can raise ORDO_PUBLIC_WRITES_PER_HOUR, or set it to 0 to \
                 turn the cap off",
                settings.writes_per_hour
            ),
        )
    };
    if !public.write_room(&op.name) {
        return Err(over_cap());
    }
    if settings.human_check {
        let refuse = |message: String| {
            count("human_check_failed");
            ApiError::for_op(ErrorCode::HumanCheckFailed, op, message)
        };
        let Some(verifier) = public.human() else {
            return Err(refuse(
                "this deployment checks that a public write comes from a person, and has no \
                 provider to check with. The deployment owner sets a Turnstile secret once \
                 with ORDO_AUTH_HUMAN_CHECK_SECRET_FILE (sign-in and public writes share \
                 it), or for public writes alone with ORDO_PUBLIC_HUMAN_SECRET_FILE (and \
                 ORDO_PUBLIC_HUMAN_PROVIDER), or turns the check off with \
                 ORDO_PUBLIC_HUMAN_CHECK=off"
                    .into(),
            ));
        };
        let token = headers
            .get(crate::public::HUMAN_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|t| !t.is_empty() && t.len() <= HUMAN_TOKEN_MAX);
        let Some(token) = token else {
            return Err(refuse(format!(
                "a public write needs a human check: send the {} widget's token in the \
                 X-Ordo-Human header",
                verifier.provider()
            )));
        };
        match verifier.verify(token.to_string()).await {
            crate::public::HumanVerdict::Pass => {}
            crate::public::HumanVerdict::Fail => {
                return Err(refuse(format!(
                    "{} did not confirm the human check. A token is good for one write; \
                     run the widget again",
                    verifier.provider()
                )));
            }
            crate::public::HumanVerdict::Unavailable(why) => {
                eprintln!(
                    "public write `{}`: the {} check could not be made ({why}); refused",
                    op.name,
                    verifier.provider()
                );
                return Err(refuse(format!(
                    "{} could not be asked to confirm the human check; nothing was written. \
                     Try again shortly",
                    verifier.provider()
                )));
            }
        }
    }
    if !public.admit_write(&op.name) {
        return Err(over_cap());
    }
    let keyed = op.public == PublicMode::WriteKeyed;
    let mut outcome = match run_call(state, op, headers, gate).await {
        Ok(o) => o,
        // A create keyed on a unique field whose value is already there
        // writes nothing and answers as a new row does (ask 31).
        Err(e) if keyed && matches!(e.code, ErrorCode::ExpectedAbsent | ErrorCode::Duplicate) => {
            count("duplicate");
            return Ok(keyed_answer(op));
        }
        Err(e) => {
            count("failed");
            return Err(e);
        }
    };
    count("written");
    if keyed {
        return Ok(keyed_answer(op));
    }
    // Write-only: each created row's id, and nothing else. The write
    // summary today is `{created, entity}`; anything a later summary adds
    // stays out of a visitor's answer until it is chosen to go in.
    if let Some(rows) = outcome.body.get_mut("rows").and_then(Value::as_array_mut) {
        for row in rows.iter_mut() {
            let id = row.get("created").cloned();
            let mut only = Map::new();
            if let Some(id) = id {
                only.insert("created".into(), id);
            }
            *row = Value::Object(only);
        }
    }
    let mut response = call_response(outcome, None);
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The one answer a public write keyed on a unique field gives, for a new
/// row and for one that was already there. It names no id, carries no rows,
/// and is built from the op alone, so nothing in it can differ between the
/// two cases (owner, 2026-10-12, ask 31).
fn keyed_answer(op: &OpSpec) -> Response {
    let mut body = Map::new();
    body.insert("op".into(), Value::String(op.name.clone()));
    body.insert("version".into(), Value::String(op.version_hash.clone()));
    if let Some(mode) = &op.mode_label {
        body.insert("mode".into(), Value::String(mode.clone()));
    }
    body.insert("rows".into(), Value::Array(Vec::new()));
    let mut response = call_response(
        CallOutcome {
            status: StatusCode::OK,
            body: Value::Object(body),
            replayed: false,
            position: None,
        },
        None,
    );
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

// ── stage 2: refinement → cursor → bind → replay → execute ──

async fn run_call(
    state: &ApiState,
    op: &OpSpec,
    headers: &HeaderMap,
    mut gate: Gate,
) -> Result<CallOutcome, ApiError> {
    // The space the executor will be handed — verified to be the one the
    // access check approved before anything else in stage 2 runs.
    let space = gate.approved_space(op)?.to_string();
    let refinement = parse_refinement(op, gate.refine_raw.take(), &state.refine_limits)?;
    let keyed_as = gate.keyed_as();
    let cursor = parse_cursor(state, &space, op, &keyed_as, gate.page_raw.take())?;
    let bound = bind::bind_params(op, &gate.params)?;

    // Idempotent replay: same caller, same request, same response, executor
    // untouched. Honored on POST and DELETE (i.e. writes and deletes) — never
    // on GET, which method enforcement has already pinned to `Read` ops.
    //
    // The key carries space and principal, and the entry carries a payload
    // fingerprint (SH-17): a cache keyed on `(op, header value)` hands one
    // principal's stored response — sealed values included — to anyone who
    // guesses the key, with the executor's row guards never running.
    let idem_key = matches!(op.kind, OpKind::Write | OpKind::Delete)
        .then(|| headers.get("idempotency-key").and_then(|v| v.to_str().ok()))
        .flatten()
        .map(str::to_string);

    // **An op may declare the header mandatory** (`bootstrap.md` §2 item 44,
    // [`OpSpec::requires_idempotency_key`]). Refused here, before anything is
    // reserved or executed, because the whole point is that this op's effect
    // cannot be made twice safely — so the call that would make it twice must
    // not reach the executor even once.
    //
    // The refusal is the caller's to fix and names the header, which is not
    // an enumeration concern: the op is one the caller already passed the
    // door for, so nothing here tells them anything they could not already
    // see in their own catalog.
    if op.requires_idempotency_key && idem_key.is_none() {
        return Err(ApiError::for_op(
            ErrorCode::MissingParam,
            op,
            "this op requires an `Idempotency-Key` header. Its effect reaches outside the              database it writes to, so a retry without one would make a second of everything              it creates and answer with the second. Nothing was created",
        ));
    }
    let slot = idem_key.map(|key| {
        let replay_key = ReplayKey {
            space: space.clone(),
            principal: keyed_as.clone(),
            op: op.name.clone(),
            key,
        };
        let fingerprint = RequestFingerprint::compute(
            &space,
            &op.name,
            &op.version_hash,
            &bound,
            refinement.as_ref(),
            cursor.as_ref(),
        );
        (replay_key, fingerprint)
    });
    // A creating op's key stays bound to its payload after the response
    // expires (Z-018): otherwise the same key with an edited payload misses a
    // lapsed cache, executes, and silently converges onto the created row —
    // with no `rev` in a creating form, no CAS exists to catch it.
    let binding = if op.creates_rows() {
        KeyBinding::Permanent
    } else {
        KeyBinding::UntilExpiry
    };
    if let Some((replay_key, fingerprint)) = &slot {
        // Reserve *before* executing (Z-012). A lookup here and a put after
        // execution would let two concurrent identical requests both miss and
        // both run.
        if let Some(outcome) =
            reserve_or_replay(state, op, replay_key, fingerprint, binding).await?
        {
            return Ok(outcome);
        }
    }

    // The executor receives the resolved principal as `$caller` — a
    // reserved `$`-key no declared parameter can shadow (SH2-1), taken from
    // the access decision like every other attribution (SH-17). Row guards
    // bind their caller slot from it; without this the executor would have
    // to re-derive a principal from caller-controlled input.
    let mut bound = bound;
    gate.bind_caller(&mut bound);
    // The lease travels with the call so the executor can record the key's
    // completion in the same transaction as the effect (AA-004's third
    // leg). The in-memory `put` below still runs — it is what makes a
    // replay verbatim — but it is no longer what makes the guarantee.
    let lease = slot.as_ref().map(|(k, f)| ReplayLease {
        key: [
            k.space.as_str(),
            k.principal.as_str(),
            k.op.as_str(),
            k.key.as_str(),
        ]
        .join("\u{1f}"),
        fingerprint: f.hex(),
    });
    match state
        .executor
        .execute(op, &space, bound, refinement, cursor, lease)
        .await
    {
        Ok(ExecOutcome::Ran(result)) => {
            let position = result.position;
            let body = success_envelope(state, &space, op, &keyed_as, result);
            if let Some((replay_key, fingerprint)) = slot {
                state
                    .replay
                    .put(
                        replay_key,
                        fingerprint,
                        StoredResponse {
                            status: StatusCode::OK.as_u16(),
                            body: body.clone(),
                        },
                        binding,
                    )
                    .await;
            }
            Ok(CallOutcome {
                status: StatusCode::OK,
                body,
                replayed: false,
                position,
            })
        }
        // The effect committed under an earlier attempt that died before
        // its response was stored. Re-running is exactly what idempotency
        // exists to prevent, so this answers rather than executes — and
        // says which case it is, because "your write already happened, we
        // no longer have the body" is a different fact from "here is your
        // body again".
        //
        // **This is `idempotency_applied`'s only producer**, and the class
        // exists because the answer is TERMINAL. It wore
        // `idempotency_in_flight` for a while, with a message that said the
        // real thing — but the message is prose and the class is the field
        // machines switch on, and `in_flight` promises a caller that asking
        // again will eventually produce the answer. Here there is nobody to
        // wait for: the effect is committed, the journal says so durably,
        // and the body went with the process that ran it. A caller polling
        // on the old class polls forever.
        //
        // `abandon` and not `put`: the reservation is released so the key
        // is not parked until its timeout, and nothing is stored. Caching
        // this answer would write a terminal 409 into the very slot a
        // stalled holder's `put` may still fill with the real body — see
        // `ErrorCode::IdempotencyApplied`'s doc for why it is deliberately
        // outside `is_exec_conflict`.
        Ok(ExecOutcome::AlreadyApplied) => {
            if let Some((replay_key, _)) = &slot {
                state.replay.abandon(replay_key).await;
            }
            Err(ApiError::for_op(
                ErrorCode::IdempotencyApplied,
                op,
                "this key's effect already committed; its response was lost with the \
                 process that ran it, so it cannot be replayed verbatim. Retrying will \
                 not change this answer",
            ))
        }
        Err(exec_err) => {
            let err = ApiError::from_exec(op, exec_err);
            if let Some((replay_key, fingerprint)) = slot {
                // Conflict outcomes are the op's result — store them too, so a
                // retried request cannot flip from `duplicate` to
                // re-execution. Everything else releases the reservation: a
                // `not_found`/`access_guard`/`internal` retry is re-decided
                // against current state, so the key must be free at once
                // rather than parked until the reservation times out.
                if err.code.is_exec_conflict() {
                    state
                        .replay
                        .put(
                            replay_key,
                            fingerprint,
                            StoredResponse {
                                status: err.code.status().as_u16(),
                                body: err.envelope(),
                            },
                            binding,
                        )
                        .await;
                } else {
                    state.replay.abandon(&replay_key).await;
                }
            }
            Err(err)
        }
    }
}

/// Reserve the idempotency key or answer from what it already holds (Z-012).
///
/// `Ok(None)` means this caller **won the reservation** and must execute, then
/// `put` or `abandon`. `Ok(Some(outcome))` means a stored response answered
/// the call. The loser of a race never executes: it waits out the winner and
/// replays, and if the wait budget elapses first it says
/// `idempotency_in_flight` rather than running a second copy of the write.
async fn reserve_or_replay(
    state: &ApiState,
    op: &OpSpec,
    replay_key: &ReplayKey,
    fingerprint: &RequestFingerprint,
    binding: KeyBinding,
) -> Result<Option<CallOutcome>, ApiError> {
    let deadline = tokio::time::Instant::now() + REPLAY_WAIT_BUDGET;
    loop {
        match state.replay.claim(replay_key, fingerprint, binding).await {
            ReplayClaim::Reserved => return Ok(None),
            ReplayClaim::Hit(stored) => {
                return Ok(Some(CallOutcome {
                    status: StatusCode::from_u16(stored.status).unwrap_or(StatusCode::OK),
                    body: stored.body,
                    replayed: true,
                    position: None,
                }));
            }
            ReplayClaim::Mismatch => {
                return Err(ApiError::for_op(
                    ErrorCode::IdempotencyMismatch,
                    op,
                    "this Idempotency-Key was used with a different request payload",
                ));
            }
            ReplayClaim::InFlight => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(ApiError::for_op(
                        ErrorCode::IdempotencyInFlight,
                        op,
                        "an identical request under this Idempotency-Key is still running — \
                         retry to receive its response",
                    ));
                }
                tokio::time::sleep(REPLAY_POLL_INTERVAL).await;
            }
            // The cache's database failed the claim. Asked again within the
            // same budget, because a blip clears; past it, the answer is the
            // database's own fault class, never `idempotency_in_flight`:
            // nobody holds this key, and that class promises somebody does.
            ReplayClaim::Unavailable(class) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(replay_unavailable(op, class));
                }
                tokio::time::sleep(REPLAY_POLL_INTERVAL).await;
            }
        }
    }
}

/// What a caller hears when the replay cache could not be asked for the
/// whole wait budget. Nothing ran and nothing was reserved, so the same
/// request may be sent again: a full or starting database is `server_busy`
/// as everywhere else, and any other database fault is `space_unavailable`,
/// both 503 with `Retry-After`.
fn replay_unavailable(op: &OpSpec, class: ordo_fault::FaultClass) -> ApiError {
    let mut down = ApiError::for_op(
        ErrorCode::SpaceUnavailable,
        op,
        "the idempotency record for this request could not be reached, so nothing ran. \
         Send the same request, with the same Idempotency-Key, again after the Retry-After wait",
    );
    down.retry_after = Some(5);
    ApiError::by_fault(
        class,
        ApiError::server_busy().with_op(op.name.clone()),
        down,
    )
}

/// The request envelope, split from whichever transport carried it.
struct Envelope {
    params: Map<String, Value>,
    refine_raw: Option<Value>,
    page_raw: Option<Value>,
    space_raw: Option<Value>,
}

/// Split the JSON body into bare-keyed params and the closed `$`-envelope
/// set (`$refine`, `$page`, `$space`). Any other `$`-key is an unknown
/// parameter.
fn split_envelope(op: &OpSpec, body: &Bytes) -> Result<Envelope, ApiError> {
    let object: Map<String, Value> = if body.is_empty() {
        Map::new()
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(Value::Object(map)) => map,
            Ok(_) => {
                return Err(ApiError::for_op(
                    ErrorCode::InvalidParam,
                    op,
                    "request body must be a JSON object of parameters",
                ));
            }
            Err(e) => {
                return Err(ApiError::for_op(
                    ErrorCode::InvalidParam,
                    op,
                    format!("request body is not valid JSON: {e}"),
                ));
            }
        }
    };

    let mut envelope = Envelope {
        params: Map::new(),
        refine_raw: None,
        page_raw: None,
        space_raw: None,
    };
    for (key, value) in object {
        match key.as_str() {
            "$refine" => envelope.refine_raw = Some(value),
            "$page" => envelope.page_raw = Some(value),
            "$space" => envelope.space_raw = Some(value),
            "$caller" => return Err(caller_supplied(op)),
            _ if key.starts_with('$') => {
                return Err(unknown_envelope_key(op, &key));
            }
            _ => {
                envelope.params.insert(key, value);
            }
        }
    }
    Ok(envelope)
}

/// The query-string counterpart of `split_envelope`, for `GET` (one-shot and
/// live) and `DELETE`. Encoding choice: `$refine` rides as URL-encoded JSON in
/// the query value (`?$refine=%7B%22pick%22...%7D`) — the envelope grammar
/// stays byte-identical to the POST body's, so one serializer serves both
/// transports; the server URL-decodes, then JSON-parses. `$page` is the
/// opaque token verbatim: it is already URL-safe base64, and requiring JSON
/// string quotes around it would add noise for nothing. `$space` likewise
/// rides verbatim, though `X-Ordo-Space` is the preferred form here — a space
/// id in a query string ends up in proxy logs and `Referer` headers. `live` is
/// the one reserved bare key and is stripped here (its value was consumed at
/// dispatch). Remaining keys coerce by declared type, then bind identically to
/// a POST body.
fn split_query(op: &OpSpec, query: &HashMap<String, String>) -> Result<Envelope, ApiError> {
    let mut params = HashMap::new();
    let mut refine_raw = None;
    let mut page_raw = None;
    let mut space_raw = None;
    for (key, value) in query {
        match key.as_str() {
            "live" => {}
            "sync" if wants_live(query) => {}
            "$refine" => {
                refine_raw = Some(serde_json::from_str::<Value>(value).map_err(|e| {
                    ApiError::for_op(
                        ErrorCode::InvalidRefinement,
                        op,
                        format!("$refine query parameter must be URL-encoded JSON: {e}"),
                    )
                })?);
            }
            "$page" => page_raw = Some(Value::String(value.clone())),
            "$space" => space_raw = Some(Value::String(value.clone())),
            "$caller" => return Err(caller_supplied(op)),
            _ if key.starts_with('$') => {
                return Err(unknown_envelope_key(op, key));
            }
            _ => {
                params.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(Envelope {
        params: bind::coerce_query(op, &params),
        refine_raw,
        page_raw,
        space_raw,
    })
}

/// `$caller` in the request envelope (`guards-conformance.md` R-G13,
/// GC067–GC068). The door resolves the principal from the credential and
/// hands it to the executor under this exact key; a client that puts one in
/// the envelope is proposing its own principal, and every guard downstream
/// would be evaluated against it.
///
/// Refused **here**, before binding, planning, or any executor work — so no
/// guard is ever evaluated, on a read or a write. It is a class of its own
/// rather than `unknown_param`, because "you may not name a parameter that"
/// and "you may not assert who you are" are different answers, and only the
/// second one is an authorization event worth a 403 in the operator's log.
///
/// This says nothing the caller did not already know (they sent the key),
/// so it does not weaken the door's one-answer rule for *credentials*
/// (§4.1): that rule keeps an attacker from learning which of token, op, or
/// membership refused them, and no `$caller` in a body reveals any of the
/// three.
fn caller_supplied(op: &OpSpec) -> ApiError {
    ApiError::for_op(
        ErrorCode::CallerSupplied,
        op,
        "`$caller` is bound by the platform from your credential and cannot be supplied \
         by the client — remove it",
    )
    .with_param("$caller")
}

fn unknown_envelope_key(op: &OpSpec, key: &str) -> ApiError {
    ApiError::for_op(
        ErrorCode::UnknownParam,
        op,
        format!("unknown envelope key `{key}` — envelope keys are $refine, $page, $space"),
    )
    .with_param(key.to_string())
}

fn parse_refinement(
    op: &OpSpec,
    raw: Option<Value>,
    limits: &refine::RefineLimits,
) -> Result<Option<Refinement>, ApiError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if op.kind != OpKind::Read {
        return Err(ApiError::for_op(
            ErrorCode::InvalidRefinement,
            op,
            "refinement applies to read ops only",
        ));
    }
    let refinement = refine::parse(op, raw)?;
    // Two gates, in the order §7.1 pins them, and both of them every time.
    //
    // `validate` is the declaration-time half: may this refinement be
    // written at all (refinable op, known fields, sealed opt-in,
    // equality-only, nothing sealed under an aggregate)? `check_cost` is the
    // request-time half: what does it cost, given what the op was registered
    // for? §7.1 says in as many words that "implementing only one half
    // leaves the hole open in one direction" — declaration-time gating alone
    // still lets an opted-in field blow the cost class, which is this call.
    //
    // The effective class it returns is the class this request is served at.
    // Nothing carries it yet: `implementation.md`'s `ordo.cost_class` span
    // attribute needs a tracing substrate the workspace does not have
    // (`ordo-host::metrics` says so at length). It is public
    // (`refine::effective_class`) so the layer that gains one can read it
    // without recomputing it, and it is deliberately not smuggled onto the
    // wire as a header the protocol never specified.
    refine::validate(op, &refinement, limits)?;
    let _effective = refine::check_cost(op, &refinement)?;
    // The venue, last: a condition only the host can answer is refused here,
    // naming the field, until host-side filtering is built (ruled
    // 2026-10-05, `refinement-default` ask 3).
    refine::check_venue(op, &refinement)?;
    Ok(Some(refinement))
}

fn parse_cursor(
    state: &ApiState,
    space: &str,
    op: &OpSpec,
    principal: &str,
    raw: Option<Value>,
) -> Result<Option<PageCursor>, ApiError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if op.kind != OpKind::Read {
        return Err(ApiError::for_op(
            ErrorCode::InvalidCursor,
            op,
            "pagination applies to read ops only",
        ));
    }
    let Some(token) = raw.as_str() else {
        return Err(ApiError::for_op(
            ErrorCode::InvalidCursor,
            op,
            "$page must be a string token",
        ));
    };
    // Authenticated against this space, this op and this principal (SH-6,
    // C4-012). `space` is the approved space, so a cursor cannot be carried
    // between two spaces the same principal belongs to.
    page::decode(&state.page_key, space, op, principal, token).map(Some)
}

// ── GET /op/{name}?live=1 — SSE live subscription (read ops only) ──

async fn live_inner(
    state: &ApiState,
    op: &OpSpec,
    gate: Gate,
    headers: &HeaderMap,
    sync: bool,
    resume: Option<String>,
) -> Result<Response, ApiError> {
    live_checks(op, &gate, sync)?;
    let space = gate.approved_space(op)?;
    let mut bound = bind::bind_params(op, &gate.params)?;
    // Same `$caller` hand-off as the one-shot path.
    gate.bind_caller(&mut bound);
    let refinement = live_refinement(state, op, gate.refine_raw.clone(), sync)?;

    if sync {
        let frames = crate::sync::start(
            state,
            op,
            space,
            bound,
            refinement,
            resume,
            Door::new(state, headers),
        )
        .await?;
        let sse = Sse::new(frames).keep_alive(KeepAlive::new().interval(HEARTBEAT).text("hb"));
        return Ok(sse.into_response());
    }

    let (subscription, initial) =
        subscribe_then_read(state, op, space, &gate.keyed_as(), bound, refinement).await?;

    // First frame: the ordinary envelope as a default `data:` event. Then
    // one `invalidate` event per bus signal — a staleness signal, never a
    // data push; the client refetches through the normal op under its own
    // scope. A lagged subscriber also gets `invalidate`: a missed signal and
    // a coalesced signal are indistinguishable by design.
    let first = stream::iter(vec![Ok::<Event, Infallible>(
        Event::default().data(initial.to_string()),
    )]);
    // The subscription is the stream's state, so the bus keeps listening
    // for exactly as long as the feed is open.
    //
    // The door is asked again before every `invalidate` and on every idle
    // tick ([`Door`]); a refusal is the feed's last frame. Signals that
    // queued behind one are answered by the same pass, so a burst of writes
    // costs one door check and one `invalidate`.
    let door = Door::new(state, headers);
    let tick = recheck_ticker(door.recheck());
    let watched = (op.clone(), space.to_string());
    let invalidations = stream::unfold(
        Some((subscription, door, tick, watched)),
        |open| async move {
            let (mut sub, door, mut tick, watched) = open?;
            loop {
                tokio::select! {
                    heard = sub.receiver.recv() => match heard {
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                            while let Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) =
                                sub.receiver.try_recv()
                            {}
                            return Some(match door.admit(&watched.0, &watched.1).await {
                                Ok(()) => (
                                    Ok(Event::default().event("invalidate").data("{}")),
                                    Some((sub, door, tick, watched)),
                                ),
                                Err(e) => (Ok(refusal_frame(&e, None)), None),
                            });
                        }
                        Err(broadcast::error::RecvError::Closed) => return None,
                    },
                    _ = tick.tick() => {
                        if let Err(e) = door.admit(&watched.0, &watched.1).await {
                            return Some((Ok(refusal_frame(&e, None)), None));
                        }
                    }
                }
            }
        },
    );
    let sse = Sse::new(first.chain(invalidations))
        .keep_alive(KeepAlive::new().interval(HEARTBEAT).text("hb"));
    Ok(sse.into_response())
}

/// What a read must be to be followed live, on either stream: live-capable,
/// at its full scope, unpaged, and syncable when a sync is asked for.
fn live_checks(op: &OpSpec, gate: &Gate, sync: bool) -> Result<(), ApiError> {
    // Method enforcement has already pinned GET to read ops; the one live
    // question left is capability.
    if !op.live_capable {
        return Err(ApiError::for_op(
            ErrorCode::NotLive,
            op,
            format!("op `{}` is not live-capable", op.name),
        ));
    }
    // A sync keys rows by the anchor's id, so an op without one is refused
    // here, naming the rule it fails (`plan-replica-feed.md` §1).
    if sync {
        crate::sync::check(op)?;
    }
    // A subscription follows the whole result, so a cursor does not apply.
    // A refinement does (`plan-refinement.md` section 7): invalidation stays
    // per op, because a refinement changes which rows the answer shows and
    // not which writes can change it. The first frame is the refined read,
    // and a refetch carries the refinement again. A signal for a row outside
    // the filter costs one refetch that returns the same rows.
    if gate.page_raw.is_some() {
        return Err(ApiError::for_op(
            ErrorCode::InvalidCursor,
            op,
            "pagination does not apply to a live subscription",
        ));
    }
    Ok(())
}

/// A live read's refinement: every gate the one-shot path runs
/// ([`parse_refinement`]), and for a sync, `pick` refused.
///
/// Ruled 2026-10-05 (`refinement-default` ask 2): a sync accepts `where` and
/// refuses `pick`. The device stores whole rows keyed by `(entity, id)`, and
/// a row held through two synced ops stays until neither holds it, so a
/// picked row would overwrite a whole one under the same key. `pick` is a
/// presentation choice the device applies locally.
fn live_refinement(
    state: &ApiState,
    op: &OpSpec,
    raw: Option<Value>,
    sync: bool,
) -> Result<Option<Refinement>, ApiError> {
    let refinement = parse_refinement(op, raw, &state.refine_limits)?;
    if sync && refinement.as_ref().is_some_and(|r| r.pick.is_some()) {
        return Err(ApiError::for_op(
            ErrorCode::InvalidRefinement,
            op,
            "a sync takes a refinement's `where` and refuses its `pick`: the device stores \
             whole rows keyed by id, so a picked row would overwrite a whole one. Sync with \
             `where` alone and pick on the device",
        )
        .with_param("pick"));
    }
    Ok(refinement)
}

/// Subscribe to the op's signals, THEN read its first envelope, so no
/// commit can slip between the snapshot and the subscription. The first
/// envelope is the refined read when the subscription carries a refinement.
async fn subscribe_then_read(
    state: &ApiState,
    op: &OpSpec,
    space: &str,
    principal: &str,
    bound: crate::spec::BoundParams,
    refinement: Option<Refinement>,
) -> Result<(crate::spec::Subscription, Value), ApiError> {
    let subscription = state.bus.subscribe_in(&op.name, space).await;
    let result = match state
        .executor
        .execute(op, space, bound, refinement, None, None)
        .await
        .map_err(|e| ApiError::from_exec(op, e))?
    {
        ExecOutcome::Ran(r) => r,
        // Unreachable without a lease, and a live subscription is a read.
        ExecOutcome::AlreadyApplied => ExecResult {
            position: None,
            rows: Vec::new(),
            next_cursor: None,
        },
    };
    Ok((
        subscription,
        success_envelope(state, space, op, principal, result),
    ))
}

// ── /live: one stream, many reads (27s) ──
//
// The design is at the top of `crate::live`. The rule these three handlers
// keep: a read added to a shared stream meets every gate it would meet as
// `GET /op/{name}?live=1`, in the same order, and a refusal answers the
// request that asked and leaves the stream alone.

/// The answer for a stream that is closed, unknown, or another token's.
/// One answer for all three, so a stream id tells a stranger nothing.
fn no_such_stream() -> ApiError {
    ApiError::new(
        ErrorCode::NotFound,
        "no open live stream by that id for this token; open a new one with GET /live",
    )
}

/// The token half of the door for a `/live` route, asked before the stream
/// is: a dead token is refused as it is everywhere else, so the answer
/// never says whether a stream exists (owner, 2026-10-25).
async fn live_token_refusal(state: &ApiState, headers: &HeaderMap) -> Option<ApiError> {
    if let Some(refusal) = token_refusal(state, headers, "").await {
        return Some(refusal);
    }
    let token = bearer(headers)?;
    if !state.access.token_known(token) {
        return Some(ApiError::new(
            ErrorCode::AccessDenied,
            "op is not callable with this token",
        ));
    }
    None
}

/// `GET /live`: open a shared stream.
///
/// A stream holds a connection before any read names an op or a space, so
/// the door asks the one question it can ask of a bare token: did this
/// deployment issue it ([`AccessChecker::token_known`])? A string nobody
/// signed is refused with the door's uniform answer and holds nothing.
async fn live_open(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let Some(token) = bearer(&headers) else {
        return recorded(
            &state,
            None,
            None,
            LIVE_GATE,
            DoorRefusal::TokenMissing,
            ApiError::new(ErrorCode::AccessDenied, "missing bearer token"),
        )
        .into_response();
    };
    if let Some(refusal) = token_refusal(&state, &headers, "").await {
        return refusal.into_response();
    }
    if !state.access.token_known(token) {
        return ApiError::new(
            ErrorCode::AccessDenied,
            "op is not callable with this token",
        )
        .into_response();
    }
    let (_, frames) = state.live_streams.open(token, Door::new(&state, &headers));
    kept_off_disk(
        Sse::new(frames)
            .keep_alive(KeepAlive::new().interval(HEARTBEAT).text("hb"))
            .into_response(),
    )
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AddRead {
    key: String,
    op: String,
    #[serde(default)]
    params: Map<String, Value>,
    /// The read's refinement, in the `$refine` shape (`{where, pick}`). The
    /// door checks it as the one-shot path does, and the first envelope is
    /// refined by it. Signals stay per op.
    #[serde(default)]
    refine: Option<Value>,
}

/// `POST /live/{stream}`: add one read to a shared stream.
async fn live_add(
    State(state): State<ApiState>,
    Path(stream_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    live_add_inner(&state, &stream_id, &headers, &body)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}

async fn live_add_inner(
    state: &ApiState,
    stream_id: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Response, ApiError> {
    use crate::live::AddRefused;

    let add: AddRead = serde_json::from_slice(body).map_err(|e| {
        ApiError::new(
            ErrorCode::InvalidParam,
            format!("a live read is {{\"key\", \"op\", \"params\", \"refine\"}}: {e}"),
        )
    })?;
    if !crate::live::well_formed_key(&add.key) {
        return Err(ApiError::new(
            ErrorCode::InvalidParam,
            format!(
                "a live read's key is 1 to {} letters, digits, `.`, `_` or `-`",
                crate::live::MAX_KEY_LEN
            ),
        )
        .with_param("key"));
    }
    // The query-string form of the params, which is what a GET carries.
    let mut query = HashMap::new();
    for (name, value) in &add.params {
        let text = match value {
            Value::Null => continue,
            Value::String(s) => s.clone(),
            Value::Bool(_) | Value::Number(_) => value.to_string(),
            _ => {
                return Err(ApiError::new(
                    ErrorCode::InvalidParam,
                    "a live read's params are strings, numbers or booleans, as in a query string",
                )
                .with_param(name.clone()));
            }
        };
        query.insert(name.clone(), text);
    }

    let Some(token) = bearer(headers) else {
        return Err(door_refusal(
            state,
            &add.op,
            None,
            named_space(headers),
            DoorRefusal::TokenMissing,
        ));
    };
    if let Some(refusal) = live_token_refusal(state, headers).await {
        return Err(refusal);
    }
    if !state.live_streams.owns(stream_id, token) {
        return Err(no_such_stream());
    }

    // The door, exactly as `dispatch` runs it for `GET /op/{name}?live=1`.
    if let Some(refusal) = token_refusal(state, headers, &add.op).await {
        return Err(refusal);
    }
    let Some(op) = state.registry.current().get(&add.op) else {
        return Err(door_refusal(
            state,
            &add.op,
            Some(token),
            named_space(headers),
            DoorRefusal::UnregisteredOp,
        ));
    };
    let mut gate = gate(state, &op, &Method::GET, &query, headers, &Bytes::new())?;
    // The refinement rides beside the params as JSON, not in the query
    // string's URL-encoded form, and meets the same gates past the door.
    gate.refine_raw = add.refine.clone().filter(|r| !r.is_null());

    // Past the door. Ordinary typed errors and version headers are safe now.
    let full = |op: &OpSpec| {
        ApiError::for_op(
            ErrorCode::QuotaExceeded,
            op,
            format!(
                "this live stream carries its most reads ({}); open another stream, or raise \
                 ORDO_LIVE_MAX_READS above {}",
                state.live_streams.settings().max_reads_per_stream,
                state.live_streams.settings().max_reads_per_stream
            ),
        )
    };
    let answer = async {
        match state.live_streams.has_room(stream_id, token, &add.key) {
            Ok(()) => {}
            Err(AddRefused::NoStream) => return Err(no_such_stream()),
            Err(AddRefused::Full) => return Err(full(&op)),
        }
        live_checks(&op, &gate, false)?;
        let space = gate.approved_space(&op)?;
        let mut bound = bind::bind_params(&op, &gate.params)?;
        gate.bind_caller(&mut bound);
        let refinement = live_refinement(state, &op, gate.refine_raw.clone(), false)?;
        let (subscription, initial) =
            subscribe_then_read(state, &op, space, &gate.keyed_as(), bound, refinement).await?;
        match state
            .live_streams
            .add(stream_id, token, &add.key, subscription, &op, space)
        {
            Ok(()) => Ok((StatusCode::OK, Json(initial)).into_response()),
            Err(AddRefused::NoStream) => Err(no_such_stream()),
            Err(AddRefused::Full) => Err(full(&op)),
        }
    }
    .await
    .unwrap_or_else(IntoResponse::into_response);
    Ok(with_version_headers(answer, &op, headers))
}

/// `DELETE /live/{stream}/{key}`: drop one read. A key the stream does not
/// carry is already dropped, so it answers the same 204.
async fn live_drop(
    State(state): State<ApiState>,
    Path((stream_id, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return recorded(
            &state,
            None,
            None,
            LIVE_GATE,
            DoorRefusal::TokenMissing,
            ApiError::new(ErrorCode::AccessDenied, "missing bearer token"),
        )
        .into_response();
    };
    if let Some(refusal) = live_token_refusal(&state, &headers).await {
        return refusal.into_response();
    }
    if state.live_streams.remove(&stream_id, token, &key) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        no_such_stream().into_response()
    }
}

// ── shared plumbing ──

/// The success envelope. The `next` token is minted for **this** space,
/// **this** op and **this** principal, so the cursor it hands back is usable
/// by nobody else, and not by this caller anywhere else (SH-6, C4-012).
fn success_envelope(
    state: &ApiState,
    space: &str,
    op: &OpSpec,
    principal: &str,
    result: ExecResult,
) -> Value {
    let mut body = Map::new();
    body.insert("op".into(), Value::String(op.name.clone()));
    body.insert("version".into(), Value::String(op.version_hash.clone()));
    if let Some(mode) = &op.mode_label {
        body.insert("mode".into(), Value::String(mode.clone()));
    }
    body.insert("rows".into(), Value::Array(result.rows));
    if let Some(cursor) = &result.next_cursor {
        body.insert(
            "next".into(),
            Value::String(page::encode(&state.page_key, space, op, principal, cursor)),
        );
    }
    Value::Object(body)
}

/// Version-skew headers (design doc §10): always announce the served hash;
/// flag `stale` when the caller pinned a different one. Soft signal — the
/// hard rejection is page tokens only.
///
/// Applied to stage-2 responses only: announcing an op's version to a caller
/// the door turned away would confirm the op exists, undoing [`door_denial`].
fn with_version_headers(mut response: Response, op: &OpSpec, request: &HeaderMap) -> Response {
    if let Ok(value) = HeaderValue::from_str(&op.version_hash) {
        response.headers_mut().insert("x-ordo-op-version", value);
    }
    let pinned = request.get("x-ordo-version").and_then(|v| v.to_str().ok());
    if pinned.is_some_and(|p| p != op.version_hash) {
        response
            .headers_mut()
            .insert("x-ordo-version-skew", HeaderValue::from_static("stale"));
    }
    response
}
