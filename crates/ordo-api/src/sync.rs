//! `GET /op/{name}?live=1&sync=1`: the replica feed (`plan-replica-feed.md`,
//! C4b).
//!
//! A sync is the live subscription in a second mode. Where `?live=1` sends
//! the op's result once and then an empty `invalidate` per bus signal, a sync
//! sends the result as rows keyed by the anchor entity's id, and after that
//! only what changed:
//!
//! - `event: load`, one per page of the result, each read at its own position
//!   with a wait between pages ([`SyncPacing`]). Every frame says how many rows
//!   have arrived and whether the load is `complete`; until it is, the device
//!   holds a partial result and knows it.
//! - `event: change`, after the load, when a bus signal leaves the result
//!   different: `upserts` for rows that are new or changed, `deletes` for
//!   the ids that left it, and `result`, the hash of the whole result after
//!   the change (C4d). A delete means "no longer in this op's result for
//!   you", whether the row was deleted, changed so it no longer matches, or
//!   hidden by a guard change (which invalidates the op, DL G16).
//! - `event: error`, once, if a read fails part-way or the door refuses;
//!   the stream then ends. The door ([`crate::router::Door`]: the token
//!   gate and the access check) is asked before the first read, before
//!   every difference pass, and on every idle tick
//!   ([`crate::LiveSettings::recheck`]).
//!
//! The subscription remembers what it sent (ruled 2026-09-15, `feed-state`):
//! a hash per row id, for the life of the connection. A reconnect is a new
//! load. The bus is subscribed before the first read, so a change made during
//! a load is heard and answered with one difference pass once the load
//! completes, and signals that queue up while a pass runs are coalesced into
//! the next one.

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::Event;
use futures::stream::{self, Stream};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc};

use crate::error::{ApiError, ErrorCode};
use crate::page::PageCursor;
use crate::refine::Refinement;
use crate::router::ApiState;
use crate::spec::{BoundParams, ExecOutcome, ExecResult, InvalidationEvent, OpSpec, Syncability};

/// How a sync paces its reads (ruled 2026-09-15, `feed-cap`: no row cap;
/// large loads arrive in paced pages). Both numbers are deployment settings
/// (`ORDO_SYNC_PAGE_ROWS`, `ORDO_SYNC_PAGE_WAIT_MS`), read once at boot by
/// the host's `RuntimeConfig::from_env`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncPacing {
    /// Rows in each `load` frame and in each page of a difference pass.
    /// The interactive read keeps the executor's own page size.
    pub page_rows: usize,
    /// The wait between one page read and the next, in a load and in a
    /// difference pass.
    pub page_wait: Duration,
}

/// `ORDO_SYNC_PAGE_ROWS`: the default, 50 rows a page.
pub const DEFAULT_PAGE_ROWS: usize = 50;
/// `ORDO_SYNC_PAGE_ROWS`: the bounds, 1 to 1,000.
pub const PAGE_ROWS_BOUNDS: (usize, usize) = (1, 1000);
/// `ORDO_SYNC_PAGE_WAIT_MS`: the default, 50 ms between pages. Not measured
/// yet (`plan-replica-feed.md` §4 owes the measurement); chosen so a long
/// load leaves the database most of every second for everyone else.
pub const DEFAULT_PAGE_WAIT_MS: u64 = 50;
/// `ORDO_SYNC_PAGE_WAIT_MS`: the bounds, 0 to 10,000.
pub const PAGE_WAIT_MS_BOUNDS: (u64, u64) = (0, 10_000);

impl Default for SyncPacing {
    fn default() -> Self {
        SyncPacing {
            page_rows: DEFAULT_PAGE_ROWS,
            page_wait: Duration::from_millis(DEFAULT_PAGE_WAIT_MS),
        }
    }
}

/// What a synced op's rows are kept by.
struct Keyed {
    entity: String,
    id_output: String,
}

impl Keyed {
    fn of(op: &OpSpec) -> Result<Keyed, ApiError> {
        match &op.sync {
            Syncability::Syncable { entity, id_output } => Ok(Keyed {
                entity: entity.clone(),
                id_output: id_output.clone(),
            }),
            Syncability::NotSyncable(rule) => Err(ApiError::for_op(
                ErrorCode::NotLive,
                op,
                format!(
                    "op `{}` cannot be synced ({}): {}",
                    op.name,
                    rule.as_str(),
                    rule.rule()
                ),
            )),
        }
    }

    fn id_of(&self, op: &OpSpec, row: &Value) -> Result<String, ApiError> {
        match row.get(&self.id_output) {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(Value::Number(n)) => Ok(n.to_string()),
            _ => Err(ApiError::for_op(
                ErrorCode::Internal,
                op,
                format!("a synced row arrived without its `{}`", self.id_output),
            )),
        }
    }
}

/// Refuse a sync of an op that cannot be synced, naming the rule it fails.
pub(crate) fn check(op: &OpSpec) -> Result<(), ApiError> {
    Keyed::of(op).map(|_| ())
}

/// Why a sync stopped sending.
enum Stop {
    /// The client went away; nothing more to say to anyone.
    Left,
    Failed(ApiError),
}

impl From<ApiError> for Stop {
    fn from(e: ApiError) -> Self {
        Stop::Failed(e)
    }
}

struct Reader {
    state: ApiState,
    op: OpSpec,
    space: String,
    bound: BoundParams,
    /// The sync's `where`, applied to every page of the load and of every
    /// difference pass. A row that starts matching is an upsert and one that
    /// stops is a delete, through the same diff as any other change. `pick`
    /// never reaches here: the door refuses it (ruled 2026-10-05).
    refine: Option<Refinement>,
    keyed: Keyed,
}

impl Reader {
    async fn page(&self, after: Option<PageCursor>) -> Result<ExecResult, ApiError> {
        let outcome = self
            .state
            .executor
            .execute_sized(
                &self.op,
                &self.space,
                self.bound.clone(),
                self.refine.clone(),
                after,
                None,
                Some(self.state.sync.page_rows),
            )
            .await
            .map_err(|e| ApiError::from_exec(&self.op, e))?;
        Ok(match outcome {
            ExecOutcome::Ran(r) => r,
            // Unreachable without a lease, and a sync is a read.
            ExecOutcome::AlreadyApplied => ExecResult {
                position: None,
                rows: Vec::new(),
                next_cursor: None,
            },
        })
    }

    /// The signed head at the position a frame's rows were read at, in the
    /// shape the SDK's `Pin` reads (C2); the current head when the executor
    /// reported no position, as a one-shot read does.
    async fn pin(&self, position: Option<u64>) -> Value {
        let pin = match position {
            Some(upto) => self.state.attest.head_at(&self.space, upto).await,
            None => self.state.attest.head(&self.space).await,
        };
        pin.map_or(Value::Null, |p| {
            json!({
                "position": p.upto,
                "head": p.head,
                "keyId": p.key_id,
                "signature": p.signature,
                "verifiableFrom": p.verifiable_from,
            })
        })
    }

    async fn wait(&self) {
        tokio::time::sleep(self.state.sync.page_wait).await;
    }
}

/// A hash of the whole result, folded from the per-row digests the
/// subscription already keeps (`plan-replica-feed.md` C4d).
///
/// Sorted by id and length-prefixed, so it names a SET of rows rather than an
/// order: two reads that page the same rows differently still agree, and no
/// pair of ids can be concatenated into the same bytes as another pair.
fn result_hash(held: &HashMap<String, [u8; 16]>) -> String {
    use std::fmt::Write as _;
    let mut pairs: Vec<(&String, &[u8; 16])> = held.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let mut h = Sha256::new();
    for (id, d) in pairs {
        h.update((id.len() as u32).to_be_bytes());
        h.update(id.as_bytes());
        h.update(d);
    }
    let out = h.finalize();
    let mut hex = String::with_capacity(32);
    for b in &out[..16] {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

fn digest(row: &Value) -> [u8; 16] {
    let d = Sha256::digest(row.to_string().as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&d[..16]);
    out
}

/// Open a sync: subscribe, read the first page (so a refusal there is an
/// ordinary HTTP error), and hand the rest to a task that sends frames until
/// the client leaves.
pub(crate) async fn start(
    state: &ApiState,
    op: &OpSpec,
    space: &str,
    bound: BoundParams,
    refine: Option<Refinement>,
    resume: Option<String>,
    door: crate::router::Door,
) -> Result<impl Stream<Item = Result<Event, Infallible>>, ApiError> {
    let keyed = Keyed::of(op)?;
    // Subscribe BEFORE the first read, as `?live=1` does: a commit between
    // the snapshot and the subscription would otherwise never be heard.
    let crate::spec::Subscription {
        receiver: signals,
        lease,
        ..
    } = state.bus.subscribe_in(&op.name, space).await;
    // Asked again here, beside the subscription: the same door every pass
    // asks, before the first row is read.
    door.admit(op, space).await?;
    let reader = Reader {
        state: state.clone(),
        op: op.clone(),
        space: space.to_string(),
        bound,
        refine,
        keyed,
    };
    let first = reader.page(None).await?;
    // A small buffer: a slow client holds the task back rather than letting
    // frames pile up in memory.
    let (tx, rx) = mpsc::channel::<Event>(4);
    tokio::spawn(run(reader, signals, lease, first, tx, resume, door));
    Ok(stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|e| (Ok(e), rx))
    }))
}

async fn run(
    reader: Reader,
    mut signals: broadcast::Receiver<InvalidationEvent>,
    // Held for as long as the feed runs; dropping it lets the bus stop
    // listening for this space's database.
    _lease: Option<Box<dyn std::any::Any + Send + Sync>>,
    first: ExecResult,
    tx: mpsc::Sender<Event>,
    resume: Option<String>,
    door: crate::router::Door,
) {
    let mut held: HashMap<String, [u8; 16]> = HashMap::new();
    let outcome = async {
        match resume {
            // A reconnect that says what it holds. If the result is the same
            // one it was given, nothing goes over the wire.
            Some(claimed) => {
                let (hash, position) = scan_result(&reader, first, &mut held).await?;
                if hash == claimed {
                    let frame = json!({
                        "entity": reader.keyed.entity,
                        "key": reader.keyed.id_output,
                        "position": position,
                        "pin": reader.pin(position).await,
                        "rows": [],
                        "loaded": held.len(),
                        "complete": true,
                        "result": hash,
                        // The device must NOT treat this as "a complete load
                        // returned nothing" and release what it holds.
                        "resumed": true,
                    });
                    tx.send(Event::default().event("load").data(frame.to_string()))
                        .await
                        .map_err(|_| Stop::Left)?;
                } else {
                    held.clear();
                    let again = reader.page(None).await?;
                    load(&reader, again, &mut held, &tx).await?;
                }
            }
            None => load(&reader, first, &mut held, &tx).await?,
        }
        let mut tick = crate::router::recheck_ticker(door.recheck());
        loop {
            tokio::select! {
                heard = signals.recv() => match heard {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                },
                // A quiet feed asks the door too, so a logout or a withdrawn
                // delegation ends it within one tick with no write.
                _ = tick.tick() => {
                    door.admit(&reader.op, &reader.space).await.map_err(Stop::Failed)?;
                    continue;
                }
                _ = tx.closed() => return Err(Stop::Left),
            }
            // Everything that queued up behind that signal is answered by the
            // same pass.
            while let Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) = signals.try_recv() {}
            // The whole door is asked again on every pass: the token gate
            // (a withdrawn delegation, a revoked share) and the access check
            // (membership, a revoked session). A feed is opened once, but
            // what admitted it can end while it runs; a pass that skipped the
            // door would keep sending rows the caller may no longer read.
            // Refused here, the feed ends with an error frame, as a new
            // connection would be.
            door.admit(&reader.op, &reader.space)
                .await
                .map_err(Stop::Failed)?;
            if let Some(frame) = diff(&reader, &mut held).await? {
                tx.send(frame).await.map_err(|_| Stop::Left)?;
            }
        }
    }
    .await;
    if let Err(Stop::Failed(e)) = outcome {
        let _ = tx.send(crate::router::refusal_frame(&e, None)).await;
    }
}

/// Page the whole result WITHOUT sending it, filling `held`, and hash it.
///
/// This is what a resume costs: one read, no rows on the wire. When the hash
/// disagrees the rows are read again and sent normally, so a changed result
/// costs two reads — the trade the owner's ruling accepts, since a reconnect
/// that finds nothing changed is the common case and pays nothing.
async fn scan_result(
    reader: &Reader,
    first: ExecResult,
    held: &mut HashMap<String, [u8; 16]>,
) -> Result<(String, Option<u64>), Stop> {
    let mut page = first;
    let mut position = None;
    loop {
        position = page.position.or(position);
        for row in &page.rows {
            held.insert(reader.keyed.id_of(&reader.op, row)?, digest(row));
        }
        let Some(after) = page.next_cursor else {
            return Ok((result_hash(held), position));
        };
        reader.wait().await;
        page = reader.page(Some(after)).await?;
    }
}

/// The load: every page of the result, paced, each frame at its position.
async fn load(
    reader: &Reader,
    first: ExecResult,
    held: &mut HashMap<String, [u8; 16]>,
    tx: &mpsc::Sender<Event>,
) -> Result<(), Stop> {
    let mut page = first;
    let mut loaded = 0usize;
    loop {
        for row in &page.rows {
            held.insert(reader.keyed.id_of(&reader.op, row)?, digest(row));
        }
        loaded += page.rows.len();
        let complete = page.next_cursor.is_none();
        let frame = json!({
            "entity": reader.keyed.entity,
            "key": reader.keyed.id_output,
            "position": page.position,
            "pin": reader.pin(page.position).await,
            "rows": page.rows,
            "loaded": loaded,
            "complete": complete,
            // Only on the last frame: before it, `held` is a prefix of the
            // result and a hash of a prefix would resume onto a partial set.
            "result": complete.then(|| result_hash(held)),
        });
        tx.send(Event::default().event("load").data(frame.to_string()))
            .await
            .map_err(|_| Stop::Left)?;
        let Some(after) = page.next_cursor else {
            return Ok(());
        };
        reader.wait().await;
        page = reader.page(Some(after)).await?;
    }
}

/// One difference pass: read the whole result again, paced, and say what
/// changed since the last frame. `None` when nothing did.
///
/// The frame carries the position of the pass's last page. A row read on an
/// earlier page that changed after it was read is a change the bus has
/// already signalled, so the next pass sends it.
async fn diff(
    reader: &Reader,
    held: &mut HashMap<String, [u8; 16]>,
) -> Result<Option<Event>, Stop> {
    let mut now: HashMap<String, [u8; 16]> = HashMap::with_capacity(held.len());
    let mut upserts = Vec::new();
    let mut position = None;
    let mut after = None;
    loop {
        let page = reader.page(after).await?;
        position = page.position.or(position);
        for row in page.rows {
            let id = reader.keyed.id_of(&reader.op, &row)?;
            let d = digest(&row);
            if held.get(&id) != Some(&d) && now.get(&id) != Some(&d) {
                upserts.push(row);
            }
            now.insert(id, d);
        }
        match page.next_cursor {
            Some(c) => after = Some(c),
            None => break,
        }
        reader.wait().await;
    }
    let mut deletes: Vec<String> = held
        .keys()
        .filter(|id| !now.contains_key(*id))
        .cloned()
        .collect();
    deletes.sort();
    *held = now;
    if upserts.is_empty() && deletes.is_empty() {
        return Ok(None);
    }
    let frame = json!({
        "entity": reader.keyed.entity,
        "key": reader.keyed.id_output,
        "position": position,
        "pin": reader.pin(position).await,
        "upserts": upserts,
        "deletes": deletes,
        // The hash of the whole result after this change (C4d), so a device
        // that reconnects resumes on what it holds now. Without it the device
        // would offer its last load's hash, and a result that changed back to
        // that set would resume onto rows the device no longer matches.
        "result": result_hash(held),
    });
    Ok(Some(
        Event::default().event("change").data(frame.to_string()),
    ))
}
