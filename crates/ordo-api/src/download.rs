//! `GET /download`: a person's own data, as one zip file.
//!
//! Owner ruling 2026-10-22 ("A person's own download"): "Yes only if allowed
//! by the app dev. There could also be filtering to determine what 'all'
//! is."
//!
//! # Who decides what is in it
//!
//! - **The app developer** marks read ops `download` in the program. Each
//!   marked op is one entity in the file: its projection is the fields, its
//!   conditions are the filter, and a blob field it answers brings the files.
//!   No op marked is the download turned off, and the refusal says how to
//!   turn it on.
//! - **The row guard** decides the rows. The door runs each marked op as the
//!   person, exactly as a call to `GET /op/<name>` would: the same access
//!   check, the same quota gate, the same executor. A file is read through
//!   the blob door's own port, which runs the row guard again. So a person
//!   gets only what they may read, and nothing here decides that a second
//!   time.
//!
//! # The format
//!
//! A zip archive. Every entry is stored, not compressed, and its sizes and
//! CRC are in its local header, so any unzip tool opens it.
//!
//! ```text
//! manifest.json                     what is in the file, and whether it is all there
//! <entity>.json                     a JSON array: one object per row, the op's fields
//! files/<entity>/<row id>/<field>   a file's bytes
//! ```
//!
//! In `<entity>.json` a blob field holds `{ "path", "type", "size" }` when
//! its file is in the archive, `{ "missing": "<why>" }` when it is not, and
//! `null` when the row has no file.
//!
//! # Streamed, and bounded
//!
//! The rows are read first, with the ops' own answers, so a refusal is an
//! ordinary refusal before any byte is sent. Then the archive is written to
//! the response as it is made: one file is held in memory at a time, and
//! nothing is written to the server's disk. Two settings bound it, both the
//! app developer's: how many downloads one person may start in an hour on
//! one node, and how many bytes one download may hold. A file past the byte
//! bound is listed as missing, with the setting named, and the manifest
//! says `"complete": false`.
//!
//! Each download is recorded on the tenant's security-event stream through
//! [`DownloadSource::record`], when it starts.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Map, Value};

use crate::blob::{BlobRequest, BlobState, BlobVerb};
use crate::error::{ApiError, ErrorCode};
use crate::router::{
    bearer, denied_after_check, door_refusal, named_space, recorded, token_refusal, ApiState,
    AGENT_PARAM, SPACE_HEADER,
};
use crate::spec::DoorRefusal;
use crate::spec::{
    AccessDecision, CallerKind, ExecOutcome, FieldTy, OpKind, OpSpec, QuotaDecision,
};
use crate::BoundParams;

/// The route.
pub const DOWNLOAD_PATH: &str = "/download";

/// The name refusals and the security record use for the download.
pub const DOWNLOAD_NAME: &str = "download";

/// The setting that opens the download to delegated tokens, by naming
/// [`DOWNLOAD_NAME`] in it. Same list as the delete ops a delegated token may
/// call.
pub const DELEGABLE_ENV: &str = "ORDO_DELEGABLE_OPS";

/// What a refusal says when no op is marked. It names how the app developer
/// turns the download on.
pub const OFF_MESSAGE: &str = "a person's own download is off in this deployment. The app \
     developer turns it on by marking read ops `download` in the program, for example \
     `op allNotes download = notes[id, title, body]`, and applying it. Each marked op is \
     one entity in the file (docs/howto-leave.md, \"A person's own download\")";

/// One marked op, and the entity it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOp {
    pub op: String,
    pub entity: String,
}

/// The platform's half: which ops are marked in the program the door
/// serves, and where a download is recorded.
pub trait DownloadSource: Send + Sync {
    /// The ops marked `download` in the serving program, in a stable order.
    fn ops(&self) -> Vec<DownloadOp>;
    /// Record that `principal` started a download of `entities` in `space`,
    /// on that tenant's security-event stream.
    fn record(&self, space: &str, principal: &str, agent: Option<&str>, entities: &[String]);
}

/// The two bounds (`ORDO_OWN_DOWNLOAD_PER_HOUR`, `ORDO_OWN_DOWNLOAD_MAX_BYTES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadLimits {
    /// Downloads one person may start in one tenant in an hour, on one node.
    pub per_hour: u32,
    /// The most bytes of files one download holds. A file past it is listed
    /// as missing.
    pub max_bytes: u64,
}

impl DownloadLimits {
    pub const DEFAULT: DownloadLimits = DownloadLimits {
        per_hour: 6,
        max_bytes: 1024 * 1024 * 1024,
    };
    /// The largest `max_bytes` may be. A zip without the 64-bit extension
    /// holds at most 4 GiB, and this archive does not write the extension.
    pub const MAX_BYTES_CAP: u64 = 4_000_000_000;
}

impl DownloadLimits {
    /// The two settings from the environment. A value that does not parse
    /// is refused at boot by `ordo-host`, through [`DownloadLimits::parse`].
    pub fn from_env() -> DownloadLimits {
        let var = |k: &str| std::env::var(k).ok();
        DownloadLimits::parse(
            var("ORDO_OWN_DOWNLOAD_PER_HOUR").as_deref(),
            var("ORDO_OWN_DOWNLOAD_MAX_BYTES").as_deref(),
        )
        .unwrap_or_default()
    }

    /// The settings over given values. `Err` names the setting and why.
    pub fn parse(
        per_hour: Option<&str>,
        max_bytes: Option<&str>,
    ) -> Result<DownloadLimits, (&'static str, String)> {
        let mut out = DownloadLimits::DEFAULT;
        if let Some(raw) = per_hour {
            out.per_hour = raw
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    (
                        "ORDO_OWN_DOWNLOAD_PER_HOUR",
                        format!(
                            "`{raw}` is not a whole number of at least 1. The download is \
                             turned off in the program, by marking no op `download`"
                        ),
                    )
                })?;
        }
        if let Some(raw) = max_bytes {
            out.max_bytes = raw
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0 && *n <= DownloadLimits::MAX_BYTES_CAP)
                .ok_or_else(|| {
                    (
                        "ORDO_OWN_DOWNLOAD_MAX_BYTES",
                        format!(
                            "`{raw}` is not a number of bytes from 1 to {}. The file is a \
                             zip without the 64-bit extension, which holds at most 4 GiB",
                            DownloadLimits::MAX_BYTES_CAP
                        ),
                    )
                })?;
        }
        Ok(out)
    }
}

impl Default for DownloadLimits {
    fn default() -> Self {
        DownloadLimits::DEFAULT
    }
}

/// Everything the route needs beside [`ApiState`].
#[derive(Clone)]
pub struct DownloadState {
    pub source: Arc<dyn DownloadSource>,
    pub limits: DownloadLimits,
    /// Open to delegated tokens: the app developer named `download` in
    /// `ORDO_DELEGABLE_OPS`. Off by default. A guest is never let in.
    pub delegated: bool,
    /// When each person's recent downloads started, per `space\u{1f}principal`.
    recent: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
}

impl DownloadState {
    pub fn new(source: Arc<dyn DownloadSource>, limits: DownloadLimits) -> Self {
        DownloadState {
            source,
            limits,
            delegated: false,
            recent: Arc::default(),
        }
    }

    /// Let delegated tokens download (the app developer named `download` in
    /// [`DELEGABLE_ENV`]).
    pub fn open_to_delegated(mut self, on: bool) -> Self {
        self.delegated = on;
        self
    }

    /// Count one download for `key`, or refuse it when the hour is full.
    fn admit(&self, key: &str) -> Result<(), ApiError> {
        let now = Instant::now();
        let hour = Duration::from_secs(3600);
        let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        // Forget people whose last download is over an hour old, so the
        // table stays as small as the hour's traffic.
        recent.retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) < hour));
        let q = recent.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= hour) {
            q.pop_front();
        }
        if q.len() >= self.limits.per_hour as usize {
            let wait = q
                .front()
                .map(|t| hour.saturating_sub(now.duration_since(*t)).as_secs().max(1))
                .unwrap_or(3600);
            let mut e = ApiError::new(
                ErrorCode::QuotaExceeded,
                format!(
                    "you have started {} downloads in the last hour, which is this \
                     deployment's limit (ORDO_OWN_DOWNLOAD_PER_HOUR). Try again in {wait} \
                     seconds",
                    self.limits.per_hour
                ),
            )
            .with_op(DOWNLOAD_NAME);
            e.retry_after = Some(wait);
            return Err(e);
        }
        q.push_back(now);
        Ok(())
    }
}

/// The download route. `blobs` is the blob door's port, when the deployment
/// serves files; without it a blob field is listed as missing.
pub fn router(state: ApiState, blobs: Option<BlobState>, download: DownloadState) -> Router {
    Router::new()
        .route(DOWNLOAD_PATH, get(handle))
        .with_state((state, blobs, download))
}

type RouteState = (ApiState, Option<BlobState>, DownloadState);

async fn handle(State((state, blobs, dl)): State<RouteState>, headers: HeaderMap) -> Response {
    match prepare(&state, &dl, &headers).await {
        Ok(plan) => respond(blobs, dl.limits, plan),
        Err(e) => e.into_response(),
    }
}

/// One entity's rows, read before any byte is sent.
struct Part {
    entity: String,
    op: String,
    blob_fields: Vec<String>,
    rows: Vec<Value>,
}

struct Plan {
    space: String,
    principal: String,
    agent: Option<String>,
    parts: Vec<Part>,
}

/// The door, then the rows. Everything that can be refused is refused here.
async fn prepare(
    state: &ApiState,
    dl: &DownloadState,
    headers: &HeaderMap,
) -> Result<Plan, ApiError> {
    let Some(token) = bearer(headers) else {
        return Err(door_refusal(
            state,
            DOWNLOAD_NAME,
            None,
            named_space(headers),
            DoorRefusal::TokenMissing,
        ));
    };
    if let Some(refusal) = token_refusal(state, headers, DOWNLOAD_NAME).await {
        return Err(refusal);
    }
    // The download is the person's own, and by default only the person's
    // (owner, 2026-10-22). Each refusal names how it can change.
    match state.access.caller_kind(token) {
        CallerKind::Person => {}
        CallerKind::Delegated if dl.delegated => {}
        CallerKind::Delegated => {
            return Err(recorded(
                state,
                Some(token),
                named_space(headers),
                DOWNLOAD_NAME,
                DoorRefusal::DownloadNotForCaller,
                ApiError::new(
                    ErrorCode::AccessDenied,
                    format!(
                        "a person's own download is closed to a delegated token, which acts \
                         for the person. The app developer opens it by naming \
                         `{DOWNLOAD_NAME}` in {DELEGABLE_ENV}"
                    ),
                )
                .with_op(DOWNLOAD_NAME),
            ));
        }
        CallerKind::Guest => {
            return Err(recorded(
                state,
                Some(token),
                named_space(headers),
                DOWNLOAD_NAME,
                DoorRefusal::DownloadNotForCaller,
                ApiError::new(
                    ErrorCode::AccessDenied,
                    "a person's own download is closed to a share guest, and no setting opens \
                     it to one. The person downloads their own data when signed in",
                )
                .with_op(DOWNLOAD_NAME),
            ));
        }
    }
    let space = headers
        .get(SPACE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| crate::router::well_formed_space(s))
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::MissingSpace,
                "a download names its tenant: send the X-Ordo-Space header",
            )
            .with_param("$space")
        })?
        .to_string();

    let marked = dl.source.ops();
    if marked.is_empty() {
        return Err(recorded(
            state,
            Some(token),
            Some(&space),
            DOWNLOAD_NAME,
            DoorRefusal::DownloadOff,
            ApiError::new(ErrorCode::AccessDenied, OFF_MESSAGE).with_op(DOWNLOAD_NAME),
        ));
    }

    // Each marked op passes the door as a call to it would. An op this
    // person may not call is left out, as its rows would be.
    let registry = state.registry.current();
    let mut admitted: Vec<(OpSpec, String)> = Vec::new();
    let mut who: Option<(String, Option<String>)> = None;
    for m in &marked {
        let Some(op) = registry.get(&m.op) else {
            continue;
        };
        if op.kind != OpKind::Read {
            continue;
        }
        match state.access.check(token, &space, &op.name) {
            AccessDecision::Allow { principal, agent } => {
                if let Some((p, _)) = &who {
                    if *p != principal {
                        return Err(ApiError::new(
                            ErrorCode::Internal,
                            "the door named two principals for one token",
                        ));
                    }
                }
                who = Some((principal, agent));
                admitted.push((op, m.entity.clone()));
            }
            AccessDecision::Deny => {}
        }
    }
    let Some((principal, agent)) = who else {
        return Err(match state.access.not_ready(token, &space) {
            Some(wait) => ApiError::tenant_not_ready(DOWNLOAD_NAME, wait),
            None if state.access.behind(token, &space) => {
                ApiError::server_busy().with_op(DOWNLOAD_NAME)
            }
            None => denied_after_check(DOWNLOAD_NAME, Some(token)),
        });
    };
    for (op, _) in &admitted {
        if let QuotaDecision::Exceeded { quantity, .. } = state.quota.check(&space, op) {
            return Err(ApiError::for_op(
                ErrorCode::QuotaExceeded,
                op,
                format!("this tenant has used its `{quantity}` quota for the period"),
            ));
        }
    }
    let keyed = match &agent {
        None => principal.clone(),
        Some(a) => format!("{principal}+agent:{a}"),
    };
    dl.admit(&format!("{space}\u{1f}{keyed}"))?;

    let mut parts = Vec::new();
    for (op, entity) in admitted {
        let rows = read_all(state, &op, &space, &principal, agent.as_deref()).await?;
        let blob_fields = op
            .output_fields
            .iter()
            .filter(|f| f.ty == FieldTy::Blob)
            .map(|f| f.name.clone())
            .collect();
        parts.push(Part {
            entity,
            op: op.name.clone(),
            blob_fields,
            rows,
        });
    }

    let entities: Vec<String> = parts.iter().map(|p| p.entity.clone()).collect();
    dl.source
        .record(&space, &principal, agent.as_deref(), &entities);
    Ok(Plan {
        space,
        principal,
        agent,
        parts,
    })
}

/// Every row of one op, page after page, as the person.
async fn read_all(
    state: &ApiState,
    op: &OpSpec,
    space: &str,
    principal: &str,
    agent: Option<&str>,
) -> Result<Vec<Value>, ApiError> {
    let mut rows = Vec::new();
    let mut page = None;
    loop {
        let mut bound = BoundParams(Map::new());
        bound
            .0
            .insert("$caller".into(), Value::String(principal.to_string()));
        if let Some(agent) = agent {
            bound
                .0
                .insert(AGENT_PARAM.into(), Value::String(agent.to_string()));
        }
        let outcome = state
            .executor
            .execute(op, space, bound, None, page.take(), None)
            .await
            .map_err(|e| ApiError::from_exec(op, e))?;
        let ExecOutcome::Ran(result) = outcome else {
            return Err(ApiError::for_op(
                ErrorCode::Internal,
                op,
                "a read answered as a replayed write",
            ));
        };
        rows.extend(result.rows);
        match result.next_cursor {
            Some(next) => page = Some(next),
            None => return Ok(rows),
        }
    }
}

/// The response: headers now, the archive as it is made.
fn respond(blobs: Option<BlobState>, limits: DownloadLimits, plan: Plan) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let name = format!("download-{}.zip", civil(now).date());
    tokio::spawn(write_archive(blobs, limits, plan, now, tx));
    let body = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|chunk| (chunk, rx))
    });
    crate::blob::stored_bytes(
        Response::builder()
            .status(StatusCode::OK)
            .header("cache-control", "no-store"),
        crate::blob::Stored::Archive {
            content_type: "application/zip",
            filename: &name,
        },
    )
    .body(Body::from_stream(body))
    .unwrap_or_else(|_| ApiError::new(ErrorCode::Internal, "download response").into_response())
}

type Sink = tokio::sync::mpsc::Sender<Result<Vec<u8>, std::io::Error>>;

/// Write the archive into `tx`. Returns early when the reader went away.
async fn write_archive(
    blobs: Option<BlobState>,
    limits: DownloadLimits,
    mut plan: Plan,
    now: u64,
    tx: Sink,
) {
    let mut zip = Zip::new(now);
    let mut used: u64 = 0;
    let mut complete = true;
    let mut listed = Vec::new();
    let parts = std::mem::take(&mut plan.parts);
    for part in parts {
        let mut files = 0u64;
        let mut rows = part.rows;
        for (i, row) in rows.iter_mut().enumerate() {
            let Some(obj) = row.as_object_mut() else {
                continue;
            };
            let id = obj
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| {
                    !s.is_empty()
                        && s.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                })
                .map(str::to_string)
                .unwrap_or_else(|| format!("row-{i}"));
            for field in &part.blob_fields {
                let Some(handle) = obj
                    .get(field)
                    .and_then(Value::as_str)
                    .filter(|h| !h.is_empty())
                else {
                    continue;
                };
                let handle = handle.to_string();
                let path = format!("files/{}/{id}/{field}", part.entity);
                let value = match fetch(&blobs, &plan, &handle).await {
                    Err(why) => {
                        complete = false;
                        json!({ "missing": why })
                    }
                    Ok((bytes, mime)) => {
                        if used.saturating_add(bytes.len() as u64) > limits.max_bytes {
                            complete = false;
                            json!({ "missing": "this file would pass the download's size limit (ORDO_OWN_DOWNLOAD_MAX_BYTES)" })
                        } else if zip.full() {
                            complete = false;
                            json!({ "missing": "the download holds at most 65000 files" })
                        } else {
                            used += bytes.len() as u64;
                            files += 1;
                            let size = bytes.len();
                            if tx.send(Ok(zip.entry(&path, &bytes))).await.is_err() {
                                return;
                            }
                            json!({ "path": path, "type": mime, "size": size })
                        }
                    }
                };
                obj.insert(field.clone(), value);
            }
        }
        let path = format!("{}.json", part.entity);
        let count = rows.len();
        let doc = serde_json::to_vec_pretty(&Value::Array(rows)).unwrap_or_default();
        used = used.saturating_add(doc.len() as u64);
        if tx.send(Ok(zip.entry(&path, &doc))).await.is_err() {
            return;
        }
        listed.push(json!({
            "entity": part.entity,
            "op": part.op,
            "path": path,
            "rows": count,
            "files": files,
        }));
    }
    let manifest = json!({
        "format": "ordo.download",
        "version": 1,
        "space": plan.space,
        "made": civil(now).stamp(),
        "complete": complete,
        "entities": listed,
    });
    let doc = serde_json::to_vec_pretty(&manifest).unwrap_or_default();
    if tx.send(Ok(zip.entry("manifest.json", &doc))).await.is_err() {
        return;
    }
    let _ = tx.send(Ok(zip.finish())).await;
}

/// One file's bytes and type, through the blob door's port, which runs the
/// row guard. `Err` is why it is not in the archive.
async fn fetch(
    blobs: &Option<BlobState>,
    plan: &Plan,
    handle: &str,
) -> Result<(Vec<u8>, String), String> {
    let Some(blobs) = blobs else {
        return Err("this deployment serves no files".into());
    };
    let req = BlobRequest {
        space: plan.space.clone(),
        principal: plan.principal.clone(),
        agent: plan.agent.clone(),
        verb: BlobVerb::Read,
        entity: None,
        field: None,
        row_id: None,
        handle_hex: Some(handle.to_string()),
        correlation: format!("download:{}:{handle}", plan.principal),
    };
    match blobs.port.read(&req, 0, None).await {
        Ok(read) => Ok((read.bytes, read.mime)),
        Err(e) => Err(format!("the file could not be read ({})", e.code.as_str())),
    }
}

// ── the zip writer ───────────────────────────────────────────────────────

/// A zip archive written entry by entry. Stored entries, UTF-8 names, sizes
/// and CRC in each local header. No 64-bit extension, so the archive stays
/// under 4 GiB ([`DownloadLimits::MAX_BYTES_CAP`]) and 65000 entries.
struct Zip {
    offset: u64,
    central: Vec<u8>,
    count: u16,
    time: u16,
    date: u16,
}

impl Zip {
    const MAX_ENTRIES: u16 = 65_000;

    fn new(now: u64) -> Self {
        let c = civil(now);
        Zip {
            offset: 0,
            central: Vec::new(),
            count: 0,
            time: ((c.hour << 11) | (c.minute << 5) | (c.second / 2)) as u16,
            date: (((c.year.saturating_sub(1980)) << 9) | (c.month << 5) | c.day) as u16,
        }
    }

    /// Room for one more file, and the manifest and entity files after it.
    fn full(&self) -> bool {
        self.count >= Self::MAX_ENTRIES - 64
    }

    fn entry(&mut self, name: &str, data: &[u8]) -> Vec<u8> {
        let name = name.as_bytes();
        let crc = crc32(data);
        let len = data.len() as u32;
        let mut local = Vec::with_capacity(30 + name.len() + data.len());
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes()); // version needed
        local.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        local.extend_from_slice(&0u16.to_le_bytes()); // stored
        local.extend_from_slice(&self.time.to_le_bytes());
        local.extend_from_slice(&self.date.to_le_bytes());
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&len.to_le_bytes());
        local.extend_from_slice(&len.to_le_bytes());
        local.extend_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name);
        local.extend_from_slice(data);

        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes()); // made by
        c.extend_from_slice(&20u16.to_le_bytes()); // needed
        c.extend_from_slice(&0x0800u16.to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes());
        c.extend_from_slice(&self.time.to_le_bytes());
        c.extend_from_slice(&self.date.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&len.to_le_bytes());
        c.extend_from_slice(&len.to_le_bytes());
        c.extend_from_slice(&(name.len() as u16).to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes()); // extra
        c.extend_from_slice(&0u16.to_le_bytes()); // comment
        c.extend_from_slice(&0u16.to_le_bytes()); // disk
        c.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        c.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        c.extend_from_slice(&(self.offset as u32).to_le_bytes());
        c.extend_from_slice(name);

        self.offset += local.len() as u64;
        self.count += 1;
        local
    }

    fn finish(self) -> Vec<u8> {
        let mut out = self.central;
        let size = out.len() as u32;
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.count.to_le_bytes());
        out.extend_from_slice(&self.count.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(self.offset as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, slot) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    let mut c = 0xffff_ffffu32;
    for b in data {
        c = table[((c ^ u32::from(*b)) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

/// A UTC calendar time, from seconds since 1970.
struct Civil {
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl Civil {
    fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
    fn stamp(&self) -> String {
        format!(
            "{}T{:02}:{:02}:{:02}Z",
            self.date(),
            self.hour,
            self.minute,
            self.second
        )
    }
}

/// Days to a date: Howard Hinnant's `civil_from_days`.
fn civil(secs: u64) -> Civil {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as u32;
    Civil {
        year,
        month,
        day,
        hour: (rem / 3600) as u32,
        minute: (rem % 3600 / 60) as u32,
        second: (rem % 60) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn civil_reads_known_dates() {
        assert_eq!(civil(0).stamp(), "1970-01-01T00:00:00Z");
        assert_eq!(civil(1_792_670_400).stamp(), "2026-10-22T12:00:00Z");
        assert_eq!(civil(951_782_400).date(), "2000-02-29");
    }

    #[test]
    fn an_archive_lists_what_it_holds() {
        let mut z = Zip::new(1_792_670_400);
        let mut out = z.entry("a.json", b"[]");
        out.extend(z.entry("files/x/1/f", b"hello"));
        out.extend(z.finish());
        // The end record counts two entries and points at the central list.
        let end = out.len() - 22;
        assert_eq!(&out[end..end + 4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(u16::from_le_bytes([out[end + 10], out[end + 11]]), 2);
        // The first local header carries its size and name.
        assert_eq!(u32::from_le_bytes([out[18], out[19], out[20], out[21]]), 2);
        assert_eq!(&out[30..36], b"a.json");
    }

    #[test]
    fn the_settings_parse_and_refuse_by_name() {
        assert_eq!(
            DownloadLimits::parse(None, None),
            Ok(DownloadLimits::DEFAULT)
        );
        let l = DownloadLimits::parse(Some("2"), Some("1000")).unwrap();
        assert_eq!((l.per_hour, l.max_bytes), (2, 1000));
        assert_eq!(
            DownloadLimits::parse(Some("0"), None).unwrap_err().0,
            "ORDO_OWN_DOWNLOAD_PER_HOUR"
        );
        assert_eq!(
            DownloadLimits::parse(None, Some("5000000000"))
                .unwrap_err()
                .0,
            "ORDO_OWN_DOWNLOAD_MAX_BYTES"
        );
    }

    #[test]
    fn the_hour_is_counted_per_person() {
        struct None_;
        impl DownloadSource for None_ {
            fn ops(&self) -> Vec<DownloadOp> {
                Vec::new()
            }
            fn record(&self, _: &str, _: &str, _: Option<&str>, _: &[String]) {}
        }
        let dl = DownloadState::new(
            Arc::new(None_),
            DownloadLimits {
                per_hour: 2,
                max_bytes: 10,
            },
        );
        assert!(dl.admit("s\u{1f}a").is_ok());
        assert!(dl.admit("s\u{1f}a").is_ok());
        let e = dl.admit("s\u{1f}a").unwrap_err();
        assert_eq!(e.code, ErrorCode::QuotaExceeded);
        assert!(e.message.contains("ORDO_OWN_DOWNLOAD_PER_HOUR"));
        assert!(dl.admit("s\u{1f}b").is_ok());
    }
}
