//! End-to-end tests through the generated router: fake registry, fake
//! executor with canned ops (read / write / non-live read / pure delete /
//! aggregate-only count / a space-echoing read), tower `oneshot` per request.
//!
//! The fake access checker denies outside `spc_01`, so the space dimension is
//! exercised end to end — binding, disagreement, cross-space denial, and the
//! space the executor is handed.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use futures::future::BoxFuture;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tower::util::ServiceExt;

use ordo_api::{
    router, AccessChecker, AccessDecision, ApiState, BoundParams, Bounds, CostClass, ErrorCode,
    ExecError, ExecResult, FieldSpec, FieldTy, MapRegistry, MemoryBus, MemoryReplayCache,
    NotSyncable, OpCost, OpExecutor, OpKind, OpSpec, PageCursor, PageKey, ParamSpec, ParamTy,
    RefineCond, RefineOp, RefineTier, Refinement, RegistryHandle, SpecError, Syncability,
};

// ── canned ops ──

fn vendor_spend_spec(version: &str) -> OpSpec {
    OpSpec {
        name: "vendorSpend".into(),
        version_hash: version.into(),
        kind: OpKind::Read,
        params: vec![
            ParamSpec {
                name: "from".into(),
                ty: ParamTy::Date,
                required: true,
                bounds: None,
            },
            ParamSpec {
                name: "min".into(),
                ty: ParamTy::Int,
                required: false,
                bounds: Some(Bounds {
                    min: None,
                    max: Some("100".into()),
                }),
            },
            ParamSpec {
                name: "payee".into(),
                ty: ParamTy::Text { max_len: Some(8) },
                required: false,
                bounds: None,
            },
            ParamSpec {
                name: "cap".into(),
                ty: ParamTy::Money { places: 2 },
                required: false,
                bounds: None,
            },
        ],
        output_fields: vendor_manifest(),
        live_capable: true,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::Basic,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }
}

/// The vendor manifest: four open fields, one sealed field the author did
/// **not** open to refinement, and one they did.
fn vendor_manifest() -> Vec<FieldSpec> {
    vec![
        FieldSpec::new("id"),
        FieldSpec::new("name"),
        FieldSpec::new("billed"),
        FieldSpec::new("month"),
        FieldSpec::sealed("bankAccount"),
        FieldSpec::sealed_refinable("taxId"),
    ]
}

/// A read whose manifest states every field's type, so the door can check a
/// condition's value against it before anything executes
/// (`plan-refinement.md` section 4).
fn typed_probe_spec() -> OpSpec {
    OpSpec {
        name: "typedProbe".into(),
        version_hash: "tp-v1".into(),
        kind: OpKind::Read,
        params: vec![],
        output_fields: vec![
            FieldSpec::new("n").with_ty(FieldTy::Int),
            FieldSpec::new("t").with_ty(FieldTy::Text),
            FieldSpec::new("m").with_ty(FieldTy::Money { places: 2 }),
            FieldSpec::new("r").with_ty(FieldTy::Decimal { places: 4 }),
            FieldSpec::new("d").with_ty(FieldTy::Date),
            FieldSpec::new("at").with_ty(FieldTy::Datetime),
            FieldSpec::new("b").with_ty(FieldTy::Bool),
            FieldSpec::new("f").with_ty(FieldTy::Blob),
            FieldSpec::new("dur").with_ty(FieldTy::Duration { unit: "ms".into() }),
        ],
        live_capable: true,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::Basic,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }
}

/// `std.E.count`'s door spec under §2.1's pairing, as `ordo-host` builds it:
/// its own `n` is the output a `pick` names and no condition may; the list's
/// field `t` is what a condition names and is not an output.
fn probe_count_spec() -> OpSpec {
    let mut n = FieldSpec::new("n").with_ty(FieldTy::Int);
    n.filter = false;
    let mut t = FieldSpec::new("t").with_ty(FieldTy::Text);
    t.output = false;
    OpSpec {
        name: "probeCount".into(),
        version_hash: "pc-v1".into(),
        kind: OpKind::Read,
        params: vec![],
        output_fields: vec![n, t],
        live_capable: true,
        sync: Syncability::NotSyncable(NotSyncable::Measure),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::Basic,
        aggregate_only: true,
        mode_label: None,
        cost: OpCost::default(),
    }
}

/// An aggregate-only op over the same manifest — `count` inherits `list`'s
/// field names, and must still refuse every sealed condition.
fn vendor_count_spec() -> OpSpec {
    OpSpec {
        name: "vendorCount".into(),
        version_hash: "vc-v1".into(),
        kind: OpKind::Read,
        params: vec![],
        output_fields: vendor_manifest(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::Basic,
        aggregate_only: true,
        mode_label: None,
        cost: OpCost::default(),
    }
}

/// The manifest of an op over an entity carrying a **sealed field with no
/// blind index** — `orders.memo`'s shape, opted into refinement at its
/// declaration. `class` is what `ordo_plan::cost::refine_field_class`
/// computed for a condition on it against *this* op's plan, which is the
/// whole of §7.1's per-field half: the same field is `bounded-host` under an
/// op that returns a page and `unbounded-host` under one that reduces.
fn paired_manifest(class: CostClass) -> Vec<FieldSpec> {
    let mut fields = vendor_manifest();
    fields.push(FieldSpec::sealed_refinable("note").with_refine_class(class));
    fields
}

/// **The op the request-time gate exists for.**
///
/// `std.E.count`'s shape under §2.1's pairing: its own output is `{ n }`, so
/// its refinement conditions validate against the *list* manifest and apply
/// **before the reduce**. Registration proved `<= 1` returned row and asked
/// for no acknowledgment — truthfully, because one row is what it returns —
/// so `unbounded_ack` is false and `refine_candidate_bounded` is false, which
/// is the gap: one row out, every row in the space examined.
///
/// `aggregate_only` is **false here on purpose**. Rule 3 would refuse a
/// sealed condition on an aggregate-only op before the cost gate ever ran,
/// and a suite that only ever tested the two together would prove nothing
/// about either. §7.1 is explicit that the halves are complementary and that
/// "implementing only one half leaves the hole open in one direction"; the
/// pairing's manifest is *borrowed*, and the producer that borrows it is not
/// necessarily the one that sets the aggregate flag. So this spec is the
/// honest worst case: rule 3 silent, rule 4 alone on duty.
fn vendor_total_spec() -> OpSpec {
    OpSpec {
        name: "vendorTotal".into(),
        version_hash: "vt-v1".into(),
        kind: OpKind::Read,
        params: vec![],
        output_fields: paired_manifest(CostClass::UnboundedHost),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::Basic,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost {
            class: CostClass::Sql,
            unbounded_ack: false,
            refine_candidate_bounded: false,
        },
    }
}

/// The same op, registered with the `unbounded` acknowledgment — a human
/// wrote `op vendorTotal unbounded = …` and meant it. §7.1 rejects the op
/// that "quietly became unbounded at request time", not the one that said so.
fn vendor_total_acked_spec() -> OpSpec {
    OpSpec {
        name: "vendorTotalAcked".into(),
        cost: OpCost {
            unbounded_ack: true,
            ..vendor_total_spec().cost
        },
        ..vendor_total_spec()
    }
}

/// A **row-shaped** op over the same sealed field: §7.1's own carve-out.
///
/// > Because the filter runs after the page window, the *candidate count*
/// > for a row-shaped op is the page, not the entity: refinement raises the
/// > **venue** (`sql` → host-side decrypt of a page) without raising
/// > **cardinality**.
///
/// So the identical condition that is refused on `vendorTotal` must be
/// SERVED here. This spec is the false-positive guard, and it is half the
/// gate: a request-time cost gate that refuses ordinary refinements breaks
/// every refining caller in the deployment.
/// A syncable read over rows a test edits: every row a stored `vendors` row,
/// keyed by `id` (`plan-replica-feed.md` §1).
fn sync_rows_spec() -> OpSpec {
    OpSpec {
        name: "syncRows".into(),
        version_hash: "sr-v1".into(),
        kind: OpKind::Read,
        params: vec![],
        output_fields: vendor_manifest(),
        live_capable: true,
        sync: Syncability::Syncable {
            entity: "vendors".into(),
            id_output: "id".into(),
        },
        refinable: RefineTier::Basic,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
        requires_idempotency_key: false,
        public: Default::default(),
    }
}

fn vendor_page_spec() -> OpSpec {
    OpSpec {
        name: "vendorPage".into(),
        version_hash: "vp-v1".into(),
        kind: OpKind::Read,
        params: vec![],
        output_fields: paired_manifest(CostClass::BoundedHost),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::Basic,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost {
            class: CostClass::BoundedHost,
            unbounded_ack: false,
            refine_candidate_bounded: true,
        },
    }
}

/// Echoes back the space the router handed the executor, and refuses to plan
/// against a caller-supplied `claim` that disagrees with it.
fn space_echo_spec() -> OpSpec {
    OpSpec {
        name: "spaceEcho".into(),
        version_hash: "se-v1".into(),
        kind: OpKind::Read,
        params: vec![ParamSpec {
            name: "claim".into(),
            ty: ParamTy::Text { max_len: Some(64) },
            required: false,
            bounds: None,
        }],
        output_fields: ["space"].map(FieldSpec::new).to_vec(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }
}

fn add_note_spec() -> OpSpec {
    OpSpec {
        name: "addNote".into(),
        version_hash: "an-v1".into(),
        kind: OpKind::Write,
        params: vec![
            ParamSpec {
                name: "txn".into(),
                ty: ParamTy::Id,
                required: true,
                bounds: None,
            },
            ParamSpec {
                name: "body".into(),
                ty: ParamTy::Text { max_len: Some(280) },
                required: true,
                bounds: None,
            },
        ],
        output_fields: ["id", "txn", "body"].map(FieldSpec::new).to_vec(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: Some("creates".into()),
        cost: OpCost::default(),
    }
}

fn remove_note_spec() -> OpSpec {
    OpSpec {
        name: "removeNote".into(),
        version_hash: "rn-v1".into(),
        kind: OpKind::Delete,
        params: vec![ParamSpec {
            name: "note".into(),
            ty: ParamTy::Id,
            required: true,
            bounds: None,
        }],
        output_fields: ["id"].map(FieldSpec::new).to_vec(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: Some("deletes".into()),
        cost: OpCost::default(),
    }
}

fn audit_trail_spec() -> OpSpec {
    OpSpec {
        name: "auditTrail".into(),
        version_hash: "at-v1".into(),
        kind: OpKind::Read,
        params: vec![],
        output_fields: ["at", "what"].map(FieldSpec::new).to_vec(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }
}

/// A registered op no test token may call — the "exists but forbidden" case
/// that must be indistinguishable from an op that does not exist.
fn owner_only_spec() -> OpSpec {
    OpSpec {
        name: "ownerOnly".into(),
        version_hash: "oo-v1".into(),
        kind: OpKind::Read,
        params: vec![],
        output_fields: ["id"].map(FieldSpec::new).to_vec(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }
}

// ── fakes ──

/// Two principals, both members of `spc_01` only, neither an owner. Stands in
/// for `ordo-identity::AccessEngine::check(claims, space, op, now)`: the
/// decision reads the space, and it names the principal it resolved.
const SPACE: &str = "spc_01";
/// A second space the same principals belong to — the cross-space cursor
/// replay in C4-012 is only reachable for a caller authorized in both.
const SPACE_TWO: &str = "spc_02";

struct FakeAccess;

/// Whether `tok-mail`'s person has added a verified email address. Only the
/// email requirement test reads or writes it.
static MAIL_ADDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[async_trait::async_trait]
impl AccessChecker for FakeAccess {
    fn check(&self, token: &str, space: &str, op: &str) -> AccessDecision {
        let principal = match token {
            "tok-ok" => "usr_ok",
            "tok-two" => "usr_two",
            "tok-mail" => "usr_mail",
            _ => return AccessDecision::Deny,
        };
        // Membership in spc_01 and spc_02 only — any other space id, whether
        // or not it exists elsewhere in the deployment, answers identically.
        if space != SPACE && space != SPACE_TWO {
            return AccessDecision::Deny;
        }
        if op == "ownerOnly" {
            return AccessDecision::Deny;
        }
        AccessDecision::Allow {
            principal: principal.into(),
            agent: None,
        }
    }

    /// The two tokens this deployment "issued"; anything else was not.
    fn token_known(&self, token: &str) -> bool {
        matches!(token, "tok-ok" | "tok-two" | "tok-mail")
    }

    /// `tok-mail`'s person needs a verified address for `addNote`, until
    /// `MAIL_ADDED` says they added one.
    async fn email_required(
        &self,
        _token: &str,
        principal: &str,
        op: &str,
    ) -> Option<ordo_api::EmailGate> {
        (principal == "usr_mail" && op == "addNote" && !MAIL_ADDED.load(AtomicOrdering::SeqCst))
            .then(|| {
                ordo_api::EmailGate::missing(
                    "addNote needs a verified email address; ORDO_EMAIL_REQUIRED",
                )
            })
    }

    /// `spc_new` is `tok-two`'s own tenant, still being set up.
    fn not_ready(&self, token: &str, space: &str) -> Option<u64> {
        (token == "tok-two" && space == NOT_READY_SPACE).then_some(7)
    }

    /// `tok-stale` stands for a deployment person whose touch is too old:
    /// denied, and allowed once they touch again.
    fn presence_required(&self, token: &str, _space: &str, _op: &str) -> bool {
        token == "tok-stale"
    }
}

/// A tenant still being set up for `tok-two`'s person.
const NOT_READY_SPACE: &str = "spc_new";

#[derive(Default)]
struct FakeExecutor {
    /// Incremented once execution begins — i.e. after the spec was cloned at
    /// dispatch, before any gate. Lets tests observe an in-flight request.
    entered: AtomicUsize,
    /// Incremented per completed execution (idempotent-replay assertions).
    calls: AtomicUsize,
    /// When set, every execute waits for a permit — lets a test hold a
    /// request in flight while it swaps the registry.
    gate: Option<Arc<Semaphore>>,
    /// The space handed to the most recent execution.
    last_space: std::sync::Mutex<Option<String>>,
    /// Answer `AlreadyApplied` instead of running: the crash-window state,
    /// where an earlier attempt's effect committed with its `$replay`
    /// journal entry and died before its response was stored. A real
    /// executor learns this from its in-transaction journal read and rolls
    /// the attempt back untouched; this double just says so.
    already_applied: bool,
    /// The rows `syncRows` serves, which a sync test edits between reads.
    sync_rows: std::sync::Mutex<Vec<Value>>,
    /// When each `syncRows` read ran, for the pacing assertions.
    sync_reads: std::sync::Mutex<Vec<std::time::Instant>>,
    /// The page size each `syncRows` read was asked for.
    sync_page_sizes: std::sync::Mutex<Vec<usize>>,
}

impl FakeExecutor {
    /// Keyset paging on id: the page after the cursor's id, and a cursor
    /// when rows remain.
    fn keyset_page(
        rows: &[Value],
        page: &Option<PageCursor>,
        size: usize,
    ) -> (Vec<Value>, Option<PageCursor>) {
        let start = match page {
            None => 0,
            Some(c) => {
                let after = c.keys.first().cloned().unwrap_or(Value::Null);
                rows.iter()
                    .position(|r| r.get("id") == Some(&after))
                    .map(|i| i + 1)
                    .unwrap_or(rows.len())
            }
        };
        let page_rows: Vec<Value> = rows.iter().skip(start).take(size).cloned().collect();
        let next_cursor = (start + page_rows.len() < rows.len())
            .then(|| {
                page_rows
                    .last()
                    .and_then(|r| r.get("id"))
                    .cloned()
                    .map(|id| PageCursor { keys: vec![id] })
            })
            .flatten();
        (page_rows, next_cursor)
    }

    fn vendor_rows() -> Vec<Value> {
        vec![
            json!({"id": "v_1", "name": "Acme",     "billed": "1200.00", "month": "2026-01"}),
            json!({"id": "v_2", "name": "Globex",   "billed": "800.00",  "month": "2026-01"}),
            json!({"id": "v_3", "name": "Initech",  "billed": "4500.00", "month": "2026-02"}),
            json!({"id": "v_4", "name": "Umbrella", "billed": null,       "month": "2026-02"}),
        ]
    }

    fn apply_refinement(rows: Vec<Value>, refine: &Refinement) -> Vec<Value> {
        rows.into_iter()
            .filter(|row| refine.where_.iter().all(|c| cond_holds(row, c)))
            .map(|row| match &refine.pick {
                None => row,
                Some(fields) => {
                    let mut picked = serde_json::Map::new();
                    for f in fields {
                        picked.insert(f.clone(), row.get(f).cloned().unwrap_or(Value::Null));
                    }
                    Value::Object(picked)
                }
            })
            .collect()
    }
}

fn cond_holds(row: &Value, cond: &RefineCond) -> bool {
    let actual = row.get(&cond.field).unwrap_or(&Value::Null);
    match cond.op {
        RefineOp::Eq => actual == &cond.value,
        RefineOp::Ne => actual != &cond.value,
        RefineOp::Lt => scalar_cmp(actual, &cond.value) == Some(Ordering::Less),
        RefineOp::Le => {
            matches!(
                scalar_cmp(actual, &cond.value),
                Some(Ordering::Less | Ordering::Equal)
            )
        }
        RefineOp::Gt => scalar_cmp(actual, &cond.value) == Some(Ordering::Greater),
        RefineOp::Ge => {
            matches!(
                scalar_cmp(actual, &cond.value),
                Some(Ordering::Greater | Ordering::Equal)
            )
        }
        RefineOp::In => cond
            .value
            .as_array()
            .is_some_and(|arr| arr.contains(actual)),
        RefineOp::IsNull => actual.is_null(),
    }
}

/// Test-grade ordering: numeric when both sides parse as numbers, string
/// comparison otherwise, null never compares (two-valued logic).
fn scalar_cmp(a: &Value, b: &Value) -> Option<Ordering> {
    let as_num = |v: &Value| match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    };
    match (as_num(a), as_num(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y),
        _ => match (a, b) {
            (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
            _ => None,
        },
    }
}

const PAGE_SIZE: usize = 2;

impl OpExecutor for FakeExecutor {
    fn execute<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: BoundParams,
        refine: Option<Refinement>,
        page: Option<PageCursor>,
        lease: Option<ordo_api::ReplayLease>,
    ) -> BoxFuture<'a, Result<ordo_api::ExecOutcome, ExecError>> {
        self.execute_sized(op, space, bound, refine, page, lease, None)
    }

    fn execute_sized<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: BoundParams,
        refine: Option<Refinement>,
        page: Option<PageCursor>,
        // This double does not honour the lease: it has no transaction to
        // put a completion record in. The router's behaviour when an
        // executor DOES honour it is covered where that executor lives.
        _lease: Option<ordo_api::ReplayLease>,
        page_rows: Option<usize>,
    ) -> BoxFuture<'a, Result<ordo_api::ExecOutcome, ExecError>> {
        Box::pin(async move {
            if self.already_applied {
                // Reached, and deliberately NOT counted in `calls`: the
                // attempt's transaction rolled back without running the
                // body, which is the property every assertion here rests
                // on.
                self.entered.fetch_add(1, AtomicOrdering::SeqCst);
                return Ok(ordo_api::ExecOutcome::AlreadyApplied);
            }
            self.run(op, space, bound, refine, page, page_rows)
                .await
                .map(ordo_api::ExecOutcome::Ran)
        })
    }
}

impl FakeExecutor {
    async fn run(
        &self,
        op: &OpSpec,
        space: &str,
        bound: BoundParams,
        refine: Option<Refinement>,
        page: Option<PageCursor>,
        page_rows: Option<usize>,
    ) -> Result<ExecResult, ExecError> {
        {
            self.entered.fetch_add(1, AtomicOrdering::SeqCst);
            if let Some(gate) = &self.gate {
                let _permit = gate.acquire().await.expect("gate open");
            }
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            // Every execution records the space it was handed, so a test can
            // assert the door's space reached this far (SH2-1 / Y-015).
            *self.last_space.lock().unwrap() = Some(space.to_string());
            match op.name.as_str() {
                "spaceEcho" => {
                    // The executor plans against the space the *door*
                    // approved. A caller-supplied space that disagrees is a
                    // typed refusal, never a silent choice of either one.
                    if let Some(claim) = bound.0.get("claim").and_then(Value::as_str) {
                        if claim != space {
                            return Err(ExecError::internal(format!(
                                "request names space `{claim}` but the door approved \
                                 `{space}`"
                            )));
                        }
                    }
                    Ok(ExecResult {
                        position: None,
                        rows: vec![json!({"space": space})],
                        next_cursor: None,
                    })
                }
                // Echo what reached the executor, so a test can see the
                // door's normalization arrive.
                "typedProbe" | "probeCount" => Ok(ExecResult {
                    position: None,
                    rows: vec![json!({ "refine": refine })],
                    next_cursor: None,
                }),
                "vendorCount" | "vendorTotal" | "vendorTotalAcked" => Ok(ExecResult {
                    position: None,
                    rows: vec![json!({"n": Self::vendor_rows().len()})],
                    next_cursor: None,
                }),
                "vendorPage" => {
                    let mut rows = Self::vendor_rows();
                    if let Some(r) = &refine {
                        rows = Self::apply_refinement(rows, r);
                    }
                    Ok(ExecResult {
                        position: None,
                        rows,
                        next_cursor: None,
                    })
                }
                "vendorSpend" => {
                    let mut rows = Self::vendor_rows();
                    if let Some(r) = &refine {
                        rows = Self::apply_refinement(rows, r);
                    }
                    let (rows, next_cursor) = Self::keyset_page(&rows, &page, PAGE_SIZE);
                    Ok(ExecResult {
                        position: None,
                        rows,
                        next_cursor,
                    })
                }
                "syncRows" => {
                    // Each read is one position further on, as successive
                    // snapshots of a changing space would be.
                    let position = {
                        let mut reads = self.sync_reads.lock().unwrap();
                        reads.push(std::time::Instant::now());
                        reads.len() as u64
                    };
                    let mut rows = self.sync_rows.lock().unwrap().clone();
                    if let Some(r) = &refine {
                        rows = Self::apply_refinement(rows, r);
                    }
                    self.sync_page_sizes
                        .lock()
                        .unwrap()
                        .push(page_rows.unwrap_or(PAGE_SIZE));
                    let (rows, next_cursor) =
                        Self::keyset_page(&rows, &page, page_rows.unwrap_or(PAGE_SIZE));
                    Ok(ExecResult {
                        position: Some(position),
                        rows,
                        next_cursor,
                    })
                }
                "addNote" => {
                    let body = bound.0.get("body").and_then(Value::as_str).unwrap_or("");
                    match body {
                        "DUP" => Err(ExecError::new(
                            ErrorCode::Duplicate,
                            "unique (txn, body) already holds",
                        )),
                        "CAS" => Err(ExecError::new(ErrorCode::CasMiss, "rev mismatch")),
                        "CONFLICT" => Err(ExecError::new(
                            ErrorCode::ConflictingIdentity,
                            "keys address different rows",
                        )),
                        // `rev: null` — create-once — and the unique key
                        // already names a row (§6 rule 9).
                        "ABSENT" => Err(ExecError::new(
                            ErrorCode::ExpectedAbsent,
                            "the key already names note_01ABC",
                        )),
                        // The addressing unique key's bound value is null,
                        // which addresses no row (§3).
                        "NULLKEY" => Err(ExecError::new(
                            ErrorCode::NullKeyAddress,
                            "`externalId` is null",
                        )),
                        // The row is absent, or outside the caller's read view
                        // — one answer for both (SH-5).
                        "MISSING" => Err(ExecError::new(
                            ErrorCode::NotFound,
                            "no transactions row with that id",
                        )),
                        // The caller may call the op; the row is outside their
                        // *write* view. Row-level, not door-level.
                        "GUARD" => Err(ExecError::new(
                            ErrorCode::AccessGuard,
                            "write guard rejected txn_01J8ZK",
                        )),
                        // The write path's own vocabulary (9g). State
                        // decided this one, so it is a conflict.
                        "CYCLE" => Err(ExecError::new(
                            ErrorCode::TreeCycle,
                            "moving `cat_2` under `cat_3` would put it inside its own subtree",
                        )),
                        // The statement decided this one, so it is not.
                        "RESERVED" => Err(ExecError::new(
                            ErrorCode::ReservedDerivedName,
                            "`rev` is written by the platform, never by an op",
                        )),
                        // A class the executor may not say: the door's own
                        // answer, which must not survive the boundary.
                        "FORGE" => Err(ExecError::new(
                            ErrorCode::AccessDenied,
                            "the executor is not the door",
                        )),
                        _ => Ok(ExecResult {
                            position: None,
                            rows: vec![json!({
                                "id": "note_01ABC",
                                "txn": bound.0.get("txn").cloned().unwrap_or(Value::Null),
                                "body": body,
                            })],
                            next_cursor: None,
                        }),
                    }
                }
                "removeNote" => Ok(ExecResult {
                    position: None,
                    // Bulk-delete shape: affected-id descriptors.
                    rows: vec![json!({"id": bound.0.get("note").cloned().unwrap_or(Value::Null)})],
                    next_cursor: None,
                }),
                "auditTrail" => Ok(ExecResult {
                    position: None,
                    rows: vec![json!({"at": "2026-01-01", "what": "space created"})],
                    next_cursor: None,
                }),
                other => Err(ExecError::internal(format!("no fake for `{other}`"))),
            }
        }
    }
}

// ── harness ──

struct Harness {
    app: Router,
    registry: Arc<RegistryHandle>,
    exec: Arc<FakeExecutor>,
    bus: Arc<MemoryBus>,
    page_key: PageKey,
    live: ordo_api::LiveStreams,
}

fn registry_with(vendor_spend_version: &str) -> Arc<MapRegistry> {
    Arc::new(
        MapRegistry::new([
            vendor_spend_spec(vendor_spend_version),
            vendor_count_spec(),
            typed_probe_spec(),
            probe_count_spec(),
            vendor_total_spec(),
            vendor_total_acked_spec(),
            vendor_page_spec(),
            space_echo_spec(),
            add_note_spec(),
            remove_note_spec(),
            audit_trail_spec(),
            owner_only_spec(),
            sync_rows_spec(),
        ])
        .expect("canned specs are well-formed"),
    )
}

fn page_key() -> PageKey {
    PageKey::from_secret(b"test-page-key")
}

fn harness_with(exec: FakeExecutor) -> Harness {
    harness_with_replay(exec, Arc::new(MemoryReplayCache::new()))
}

/// Same harness, with the replay cache supplied — tests that drive the
/// idempotency clock need to hold a handle on it.
fn harness_with_replay(exec: FakeExecutor, replay: Arc<MemoryReplayCache>) -> Harness {
    harness_full(
        exec,
        replay,
        ordo_api::LiveStreams::default(),
        paced(PAGE_SIZE, Duration::from_millis(50)),
    )
}

/// A sync pacing with the numbers a test chooses.
fn paced(page_rows: usize, page_wait: Duration) -> ordo_api::SyncPacing {
    ordo_api::SyncPacing {
        page_rows,
        page_wait,
    }
}

/// The harness with the sync's pacing chosen by the test.
fn harness_sync(exec: FakeExecutor, pacing: ordo_api::SyncPacing) -> Harness {
    harness_full(
        exec,
        Arc::new(MemoryReplayCache::new()),
        ordo_api::LiveStreams::default(),
        pacing,
    )
}

/// The harness with the shared live streams' settings chosen by the test.
fn harness_live(settings: ordo_api::LiveSettings) -> Harness {
    harness_full(
        FakeExecutor::default(),
        Arc::new(MemoryReplayCache::new()),
        ordo_api::LiveStreams::new(settings),
        paced(PAGE_SIZE, Duration::from_millis(50)),
    )
}

fn harness_full(
    exec: FakeExecutor,
    replay: Arc<dyn ordo_api::idempotency::ReplayCache>,
    live: ordo_api::LiveStreams,
    pacing: ordo_api::SyncPacing,
) -> Harness {
    let registry = Arc::new(RegistryHandle::new(registry_with("vs-v1")));
    let exec = Arc::new(exec);
    let bus = Arc::new(MemoryBus::new());
    let state = ApiState {
        registry: registry.clone(),
        executor: exec.clone(),
        access: Arc::new(FakeAccess),
        quota: Arc::new(ordo_api::NoQuota),
        bus: bus.clone(),
        replay,
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: page_key(),
        sync: pacing,
        live_streams: live.clone(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        app: router(state),
        registry,
        exec,
        bus,
        page_key: page_key(),
        live,
    }
}

fn harness() -> Harness {
    harness_with(FakeExecutor::default())
}

fn post_op(name: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/op/{name}"))
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// One-shot read: params ride the query string (pass it pre-encoded).
fn get_op(name: &str, query: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/op/{name}?{query}"))
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .body(Body::empty())
        .unwrap()
}

/// Pure delete: params ride the query string, like GET.
fn delete_op(name: &str, query: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(format!("/op/{name}?{query}"))
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .body(Body::empty())
        .unwrap()
}

/// Minimal URL-safe base64 (no padding), for hand-building page tokens the
/// platform never minted.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let indices = [n >> 18 & 63, n >> 12 & 63, n >> 6 & 63, n & 63];
        for (i, idx) in indices.iter().enumerate() {
            if i <= chunk.len() {
                out.push(ALPHABET[*idx as usize] as char);
            }
        }
    }
    out
}

/// Minimal percent-encoder for query values (JSON refinements, spaces, …).
fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, body)
}

fn error_code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or("<no code>")
}

// ── read path ──

#[tokio::test]
async fn get_read_happy_path_coerces_query_params() {
    let h = harness();
    // Query-string params by declared type: a date, a money decimal string,
    // and an int that must coerce before its bound check.
    let (status, headers, body) = send(
        &h.app,
        get_op("vendorSpend", "from=2026-01-01&cap=1500.00&min=42"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["op"], "vendorSpend");
    assert_eq!(body["version"], "vs-v1");
    assert_eq!(body["rows"][0]["name"], "Acme");
    assert_eq!(headers.get("x-ordo-op-version").unwrap(), "vs-v1");
    assert!(headers.get("x-ordo-version-skew").is_none());

    // A malformed money value fails in the binder, same as a POST would.
    let (status, _, body) = send(&h.app, get_op("vendorSpend", "from=2026-01-01&cap=lots")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_param");
    assert_eq!(body["error"]["param"], "cap");
}

/// A `money(2)` parameter takes major units with at most two places (27ak).
/// `"29.999"` would have to round to be stored, so the door refuses it
/// before anything is planned; a trailing zero names no value and fits.
#[tokio::test]
async fn the_door_refuses_money_finer_than_its_places() {
    let h = harness();
    let (status, _, body) = send(&h.app, get_op("vendorSpend", "from=2026-01-01&cap=29.999")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "invalid_param");
    assert_eq!(body["error"]["param"], "cap");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("money(2)"),
        "{body}"
    );
    let (status, _, body) = send(&h.app, get_op("vendorSpend", "from=2026-01-01&cap=29.990")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// An unknown op and a forbidden op must be indistinguishable (C7): otherwise
/// any token enumerates the deployment's op inventory by probing names.
#[tokio::test]
async fn unknown_op_is_indistinguishable_from_forbidden() {
    let h = harness();
    let (status, _, body) = send(&h.app, post_op("noSuchOp", json!({}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");

    // A real op the token may not call answers identically — same status, same code.
    let request = Request::builder()
        .method("GET")
        .uri("/op/vendorSpend?from=2026-01-01")
        .header("authorization", "Bearer tok-wrong")
        .header("x-ordo-space", SPACE)
        .body(Body::empty())
        .unwrap();
    let (status2, _, body2) = send(&h.app, request).await;
    assert_eq!(status2, status);
    assert_eq!(error_code(&body2), error_code(&body));
}

// ── param validation, each error code ──

#[tokio::test]
async fn unknown_param_lists_valid_names() {
    let h = harness();
    let (status, _, body) = send(&h.app, get_op("vendorSpend", "from=2026-01-01&bogus=1")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "unknown_param");
    assert_eq!(body["error"]["param"], "bogus");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("from") && message.contains("min") && message.contains("payee"));
}

#[tokio::test]
async fn missing_required_param_is_named() {
    let h = harness();
    let (status, _, body) = send(&h.app, get_op("vendorSpend", "")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "missing_param");
    assert_eq!(body["error"]["param"], "from");
}

#[tokio::test]
async fn type_mismatch_is_invalid_param() {
    let h = harness();
    // A non-date string fails loudly, as does a date that never existed.
    let (status, _, body) = send(&h.app, get_op("vendorSpend", "from=20260101")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_param");
    assert_eq!(body["error"]["param"], "from");

    let (_, _, body) = send(&h.app, get_op("vendorSpend", "from=2026-13-01")).await;
    assert_eq!(error_code(&body), "invalid_param");
}

#[tokio::test]
async fn bound_violations() {
    let h = harness();
    // Int above its declared max.
    let (status, _, body) = send(&h.app, get_op("vendorSpend", "from=2026-01-01&min=200")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "bound_violation");
    assert_eq!(body["error"]["param"], "min");

    // Text over its declared length.
    let (_, _, body) = send(
        &h.app,
        get_op(
            "vendorSpend",
            &format!("from=2026-01-01&payee={}", urlenc("far too long a name")),
        ),
    )
    .await;
    assert_eq!(error_code(&body), "bound_violation");
    assert_eq!(body["error"]["param"], "payee");
}

// ── access ──

#[tokio::test]
async fn access_denied() {
    let h = harness();
    // Wrong token.
    let request = Request::builder()
        .method("GET")
        .uri("/op/vendorSpend?from=2026-01-01")
        .header("authorization", "Bearer tok-wrong")
        .header("x-ordo-space", SPACE)
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");

    // No token at all, with the space named. (With neither, the door answers
    // `access_denied` as well: see `tokenless_real_and_unknown_ops_answer_alike`.)
    let request = Request::builder()
        .method("GET")
        .uri("/op/vendorSpend?from=2026-01-01")
        .header("x-ordo-space", SPACE)
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");
    // Executor never ran: the check is at the door.
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 0);
}

/// No token and no space: a real op and an unknown one get the same answer,
/// the same status, headers and body, apart from the op name each echoes.
#[tokio::test]
async fn tokenless_real_and_unknown_ops_answer_alike() {
    let h = harness();
    let ask = |name: &'static str| {
        let app = h.app.clone();
        async move {
            let request = Request::builder()
                .method("GET")
                .uri(format!("/op/{name}"))
                .body(Body::empty())
                .unwrap();
            send(&app, request).await
        }
    };
    let (real_status, real_headers, real_body) = ask("vendorSpend").await;
    let (gone_status, gone_headers, gone_body) = ask("noSuchOp").await;
    assert_eq!(real_status, StatusCode::FORBIDDEN);
    assert_eq!(real_status, gone_status);
    assert_eq!(error_code(&real_body), "access_denied");
    // The length follows the echoed name; every other header is the same.
    let without_length = |h: &HeaderMap| {
        let mut h = h.clone();
        h.remove("content-length");
        h
    };
    assert_eq!(without_length(&real_headers), without_length(&gone_headers));
    let real = real_body.to_string().replace("vendorSpend", "OP");
    let gone = gone_body.to_string().replace("noSuchOp", "OP");
    assert_eq!(real, gone);
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 0);
}

// ── method enforcement (kind mirrored onto HTTP) ──

#[tokio::test]
async fn post_on_read_op_is_method_mismatch() {
    let h = harness();
    // No auth header on purpose: the method gate fires before access check —
    // a 403 here would mean the pipeline order broke.
    let request = Request::builder()
        .method("POST")
        .uri("/op/vendorSpend")
        .body(Body::from(json!({"from": "2026-01-01"}).to_string()))
        .unwrap();
    let (status, headers, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(error_code(&body), "method_mismatch");
    assert_eq!(headers.get("allow").unwrap(), "GET");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("read") && message.contains("GET"),
        "message: {message}"
    );
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn get_on_write_op_is_method_mismatch() {
    let h = harness();
    let (status, headers, body) = send(&h.app, get_op("addNote", "txn=txn_01J8ZK&body=hi")).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(error_code(&body), "method_mismatch");
    assert_eq!(headers.get("allow").unwrap(), "POST");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("write") && message.contains("POST"),
        "message: {message}"
    );
}

#[tokio::test]
async fn post_on_delete_op_is_method_mismatch() {
    let h = harness();
    let (status, headers, body) =
        send(&h.app, post_op("removeNote", json!({"note": "note_01ABC"}))).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(error_code(&body), "method_mismatch");
    assert_eq!(headers.get("allow").unwrap(), "DELETE");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("delete") && message.contains("DELETE"),
        "message: {message}"
    );
}

// ── refinement ──

#[tokio::test]
async fn refinement_via_get_query_happy_path() {
    let h = harness();
    // $refine rides the query string as URL-encoded JSON on GET reads.
    let refine = json!({
        "where": [{"field": "month", "op": "eq", "value": "2026-01"}],
        "pick": ["name", "billed"]
    });
    let (status, _, body) = send(
        &h.app,
        get_op(
            "vendorSpend",
            &format!("from=2026-01-01&$refine={}", urlenc(&refine.to_string())),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["rows"],
        json!([
            {"name": "Acme", "billed": "1200.00"},
            {"name": "Globex", "billed": "800.00"},
        ])
    );
    assert!(body.get("next").is_none());
}

#[tokio::test]
async fn refinement_unknown_field_is_named() {
    let h = harness();
    let refine = json!({"where": [{"field": "total", "op": "gt", "value": 5}]});
    let (status, _, body) = send(
        &h.app,
        get_op(
            "vendorSpend",
            &format!("from=2026-01-01&$refine={}", urlenc(&refine.to_string())),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_refinement");
    assert_eq!(body["error"]["param"], "total");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("billed"));
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn refinement_rejected_when_not_refinable() {
    let h = harness();
    let refine = json!({"pick": ["at"]});
    let (status, _, body) = send(
        &h.app,
        get_op(
            "auditTrail",
            &format!("$refine={}", urlenc(&refine.to_string())),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_refinement");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not refinable"));
}

// ── pagination ──

#[tokio::test]
async fn pagination_round_trip() {
    let h = harness();
    let (_, _, first) = send(&h.app, get_op("vendorSpend", "from=2026-01-01")).await;
    assert_eq!(first["rows"].as_array().unwrap().len(), 2);
    let token = first["next"].as_str().expect("next token").to_string();

    // The token is URL-safe base64 — it rides the query string verbatim.
    let (_, _, second) = send(
        &h.app,
        get_op("vendorSpend", &format!("from=2026-01-01&$page={token}")),
    )
    .await;
    assert_eq!(second["rows"][0]["name"], "Initech");
    assert_eq!(second["rows"][1]["name"], "Umbrella");
    assert!(second.get("next").is_none(), "last page carries no token");
}

#[tokio::test]
async fn pagination_version_skew_and_garbage() {
    let h = harness();
    // A genuinely minted token whose op has since been redeployed: the MAC
    // authenticates, the version does not, so the caller is told to restart
    // from page one rather than being accused of forging.
    let stale = ordo_api::page::encode(
        &h.page_key,
        SPACE,
        &vendor_spend_spec("vs-v0"),
        "usr_ok",
        &PageCursor {
            keys: vec![json!("v_2")],
        },
    );
    let (status, _, body) = send(
        &h.app,
        get_op("vendorSpend", &format!("from=2026-01-01&$page={stale}")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "version_skew");

    // Undecodable token: distinct code.
    let (status, _, body) = send(
        &h.app,
        get_op(
            "vendorSpend",
            &format!("from=2026-01-01&$page={}", urlenc("!!not-base64!!")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_cursor");
}

// ── page tokens are authenticated and principal-bound (SH-6) ──

#[tokio::test]
async fn forged_cursor_is_rejected() {
    let h = harness();
    // A caller who mints their own keyset position — the binary-search oracle
    // round 1 named — never gets past the MAC.
    let payload = base64url(&json!({"v": "vs-v1", "k": ["v_1"]}).to_string().into_bytes());
    let forged = format!("{payload}.{}", base64url(&[0u8; 32]));
    let (status, _, body) = send(
        &h.app,
        get_op("vendorSpend", &format!("from=2026-01-01&$page={forged}")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_cursor");
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        0,
        "no execution behind a forged cursor"
    );

    // Tampering with the payload of a real token breaks it too: the platform
    // issued *that* position, not this one.
    let real = ordo_api::page::encode(
        &h.page_key,
        SPACE,
        &vendor_spend_spec("vs-v1"),
        "usr_ok",
        &PageCursor {
            keys: vec![json!("v_1")],
        },
    );
    let tampered = format!("{payload}.{}", real.split_once('.').unwrap().1);
    let (_, _, body) = send(
        &h.app,
        get_op("vendorSpend", &format!("from=2026-01-01&$page={tampered}")),
    )
    .await;
    assert_eq!(error_code(&body), "invalid_cursor");
}

#[tokio::test]
async fn cursor_is_bound_to_its_principal_and_its_op() {
    let h = harness();
    // Page one as usr_ok; the token it hands back is usr_ok's.
    let (_, _, first) = send(&h.app, get_op("vendorSpend", "from=2026-01-01")).await;
    let token = first["next"].as_str().expect("next token").to_string();

    // The same token presented by a different principal is unreadable — a
    // cursor that leaks through a proxy log or a Referer header is not a
    // bearer credential.
    let request = Request::builder()
        .method("GET")
        .uri(format!("/op/vendorSpend?from=2026-01-01&$page={token}"))
        .header("authorization", "Bearer tok-two")
        .header("x-ordo-space", SPACE)
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_cursor");

    // And a cursor minted for one op is not currency at another, even when
    // the payload would parse there: the op name is inside the MAC.
    let cross_op = ordo_api::page::encode(
        &h.page_key,
        SPACE,
        &vendor_spend_spec("vc-v1"),
        "usr_ok",
        &PageCursor {
            keys: vec![json!("v_1")],
        },
    );
    let (_, _, body) = send(&h.app, get_op("vendorCount", &format!("$page={cross_op}"))).await;
    assert_eq!(error_code(&body), "invalid_cursor");

    // The rightful owner still pages.
    let (status, _, body) = send(
        &h.app,
        get_op("vendorSpend", &format!("from=2026-01-01&$page={token}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"][0]["name"], "Initech");
}

#[tokio::test]
async fn cursor_is_bound_to_its_space() {
    let h = harness();
    // usr_ok is a member of both spaces, so the door lets this request
    // through; only the cursor's own authenticator stands between a caller
    // and a seek to a position no cursor in *this* space ever authorized.
    let (_, _, first) = send(&h.app, get_op("vendorSpend", "from=2026-01-01")).await;
    let token = first["next"].as_str().expect("next token").to_string();

    let carried = Request::builder()
        .method("GET")
        .uri(format!("/op/vendorSpend?from=2026-01-01&$page={token}"))
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE_TWO)
        .body(Body::empty())
        .unwrap();
    let before = h.exec.calls.load(AtomicOrdering::SeqCst);
    let (status, _, body) = send(&h.app, carried).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_cursor");
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        before,
        "a cursor from another space must not reach the executor"
    );

    // And the token is still good where it was minted.
    let (status, _, body) = send(
        &h.app,
        get_op("vendorSpend", &format!("from=2026-01-01&$page={token}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"][0]["name"], "Initech");
}

// ── writes ──

#[tokio::test]
async fn write_returns_written_row_and_mode() {
    let h = harness();
    let (status, _, body) = send(
        &h.app,
        post_op("addNote", json!({"txn": "txn_01J8ZK", "body": "checked"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mode"], "creates");
    assert_eq!(body["rows"][0]["id"], "note_01ABC");
    assert_eq!(body["rows"][0]["body"], "checked");
}

#[tokio::test]
async fn exec_errors_pass_through_typed() {
    let h = harness();
    for (trigger, code) in [
        ("DUP", "duplicate"),
        ("CAS", "cas_miss"),
        ("CONFLICT", "conflicting_identity"),
        // §6 rule 9's create-once refusal. It was in the inventory with
        // nothing able to raise it until converge-or-create existed.
        ("ABSENT", "expected_absent"),
    ] {
        let (status, _, body) = send(
            &h.app,
            post_op("addNote", json!({"txn": "txn_01J8ZK", "body": trigger})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error_code(&body), code);
        assert_eq!(body["error"]["op"], "addNote");
    }

    // `null_key_address` is 400, not 409: nothing about the stored data
    // decided it — the request addressed no row, and re-sending the same
    // payload will be refused for the same reason forever.
    let (status, _, body) = send(
        &h.app,
        post_op("addNote", json!({"txn": "txn_01J8ZK", "body": "NULLKEY"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "null_key_address");
}

/// 9g: the write path's own vocabulary reaches the envelope, and the
/// status divides on **what refused it**, not on convenience.
#[tokio::test]
async fn write_path_classes_reach_the_envelope_with_the_right_family() {
    let h = harness();

    // State refused it — reparent the child and the same call lands.
    let (status, _, body) = send(
        &h.app,
        post_op("addNote", json!({"txn": "txn_01J8ZK", "body": "CYCLE"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "tree_cycle");
    assert_eq!(body["error"]["op"], "addNote");

    // The statement refused it — no data will ever make it land, so it is
    // not a conflict with anything.
    let (status, _, body) = send(
        &h.app,
        post_op("addNote", json!({"txn": "txn_01J8ZK", "body": "RESERVED"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "reserved_derived_name");

    // And the door's word does not survive the boundary: an executor that
    // answers `access_denied` is a bug in the executor, reported as one,
    // never a 403 the door did not issue.
    let (status, _, body) = send(
        &h.app,
        post_op("addNote", json!({"txn": "txn_01J8ZK", "body": "FORGE"})),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error_code(&body), "internal");
}

/// 9q: `$caller` is bound by the platform from the credential. A client
/// that supplies one is proposing its own principal, and every guard
/// downstream would be evaluated against it — so it is refused at the
/// door, on both transports, before binding or planning.
#[tokio::test]
async fn a_client_supplied_caller_is_refused_before_anything_runs() {
    let h = harness();

    let (status, _, body) = send(
        &h.app,
        post_op(
            "addNote",
            json!({"txn": "txn_01J8ZK", "body": "x", "$caller": "mem_9"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "caller_supplied");
    assert_eq!(body["error"]["param"], "$caller");
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        0,
        "nothing executed"
    );

    let (status, _, body) = send(&h.app, get_op("vendorCount", "$caller=mem_9")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "caller_supplied");
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        0,
        "not on the read path either"
    );
}

#[tokio::test]
async fn idempotent_write_replays_without_reexecution() {
    let h = harness();
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-123")
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": "once"}).to_string(),
            ))
            .unwrap()
    };
    let (status1, headers1, body1) = send(&h.app, request()).await;
    let (status2, headers2, body2) = send(&h.app, request()).await;
    assert_eq!(status1, StatusCode::OK);
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(body1, body2);
    assert!(headers1.get("x-ordo-idempotent-replay").is_none());
    assert_eq!(headers2.get("x-ordo-idempotent-replay").unwrap(), "true");
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        1,
        "executor ran exactly once"
    );

    // A different key executes again.
    let request3 = Request::builder()
        .method("POST")
        .uri("/op/addNote")
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .header("idempotency-key", "k-456")
        .body(Body::from(
            json!({"txn": "txn_01J8ZK", "body": "once"}).to_string(),
        ))
        .unwrap();
    send(&h.app, request3).await;
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 2);
}

/// The crash window, named on the wire: an effect that **already
/// committed** whose response body died with the process that stored it.
///
/// The `$replay` journal rides the write's own transaction, so the retry's
/// in-transaction read answers `Applied` and the attempt rolls back
/// untouched — nothing re-executes, which is the correctness property and
/// it is not what this test is about. This test is about the *class*. The
/// answer wore `idempotency_in_flight` for a while, with a message that
/// said the real thing; but the message is prose and the class is the field
/// a client switches on, and `in_flight` tells that client to ask again.
/// Nobody is running. Asking again forever is the bug the name caused.
#[tokio::test]
async fn a_crash_window_retry_is_told_the_effect_applied_and_not_to_retry() {
    let h = harness_with(FakeExecutor {
        already_applied: true,
        ..Default::default()
    });
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-crashed")
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": "committed, body lost"}).to_string(),
            ))
            .unwrap()
    };

    let (status, headers, body) = send(&h.app, request()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        error_code(&body),
        "idempotency_applied",
        "and specifically not `idempotency_in_flight`, which promises the \
         caller that asking again will change the answer; nothing is \
         running, so it never will"
    );
    assert_eq!(body["error"]["op"], "addNote");
    assert!(
        headers.get("x-ordo-idempotent-replay").is_none(),
        "nothing was replayed — that is the whole complaint"
    );
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        0,
        "the body did not run: the committed effect is not re-executed"
    );
    assert_eq!(h.exec.entered.load(AtomicOrdering::SeqCst), 1);

    // Terminal means STABLE, not just refused once. The reservation is
    // released rather than stored, so the second attempt reaches the
    // executor and gets the same answer from the journal — never
    // `idempotency_in_flight`, which is what a parked reservation would
    // have produced, and never a re-execution.
    let (status, _, body) = send(&h.app, request()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "idempotency_applied");
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        0,
        "still nothing ran"
    );
}

/// The other direction, which is the one that must not regress: when the
/// response body IS in the store, the retry gets it — 200, verbatim, with
/// the replay header — and never meets the new class.
///
/// Worth its own test rather than trusting the replay test above, because
/// the failure this catches is a router that answers `idempotency_applied`
/// whenever a key has been seen before. That would turn every ordinary
/// retry into a terminal refusal, which is a worse outage than the crash
/// window it was minted for.
#[tokio::test]
async fn a_verbatim_replay_never_wears_the_applied_class() {
    let h = harness();
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-body-survived")
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": "committed, body stored"}).to_string(),
            ))
            .unwrap()
    };

    let (status1, _, body1) = send(&h.app, request()).await;
    let (status2, headers2, body2) = send(&h.app, request()).await;
    assert_eq!(status1, StatusCode::OK);
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(body1, body2, "the stored body, byte for byte");
    assert_eq!(headers2.get("x-ordo-idempotent-replay").unwrap(), "true");
    assert!(
        body2.get("error").is_none(),
        "a replay is an answer, not a refusal: {body2}"
    );
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        1,
        "the body ran once and the retry replayed it"
    );
}

/// Z-012: the retry that arrives *while the first attempt is still running*.
/// Lookup → execute → put lets both miss and both execute; the reservation
/// makes the loser wait and replay instead.
#[tokio::test]
async fn concurrent_identical_requests_execute_the_body_exactly_once() {
    let gate = Arc::new(Semaphore::new(0));
    let h = harness_with(FakeExecutor {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-race")
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": "once"}).to_string(),
            ))
            .unwrap()
    };

    // Both in flight before either can complete: the winner is parked inside
    // the executor, the loser is parked on the winner's reservation.
    let a = tokio::spawn({
        let app = h.app.clone();
        async move { send(&app, request()).await }
    });
    let b = tokio::spawn({
        let app = h.app.clone();
        async move { send(&app, request()).await }
    });
    timeout(Duration::from_secs(2), async {
        while h.exec.entered.load(AtomicOrdering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("one request reached the executor");
    gate.add_permits(16);

    let (status_a, headers_a, body_a) = a.await.unwrap();
    let (status_b, headers_b, body_b) = b.await.unwrap();
    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(status_b, StatusCode::OK);
    assert_eq!(
        h.exec.entered.load(AtomicOrdering::SeqCst),
        1,
        "the loser never reached the executor at all"
    );
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        1,
        "the body ran exactly once"
    );
    assert_eq!(
        body_a, body_b,
        "the loser got the winner's response, byte for byte"
    );
    let replayed = [&headers_a, &headers_b]
        .iter()
        .filter(|h| h.get("x-ordo-idempotent-replay").is_some())
        .count();
    assert_eq!(replayed, 1, "exactly one of the two is a replay");
}

/// Z-012: a different payload under a key somebody else is *still executing*
/// is a mismatch, not a wait — waiting would answer a different question.
#[tokio::test]
async fn a_changed_payload_against_an_in_flight_key_is_a_mismatch() {
    let gate = Arc::new(Semaphore::new(0));
    let h = harness_with(FakeExecutor {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let post = |body: &str| {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-inflight")
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": body}).to_string(),
            ))
            .unwrap()
    };

    let winner = tokio::spawn({
        let app = h.app.clone();
        async move { send(&app, post("first")).await }
    });
    timeout(Duration::from_secs(2), async {
        while h.exec.entered.load(AtomicOrdering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the winner reached the executor");

    let (status, _, body) = send(&h.app, post("second")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "idempotency_mismatch");

    gate.add_permits(16);
    assert_eq!(winner.await.unwrap().0, StatusCode::OK);
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 1);
}

/// Z-012, the give-up branch: a loser whose wait budget runs out answers
/// `idempotency_in_flight` (409). It must never fall through to execution —
/// executing after the wait is the double-write all of this exists to stop.
/// Time is virtual here, so the budget elapses without the test sleeping.
#[tokio::test(start_paused = true)]
async fn a_loser_that_outwaits_its_budget_never_executes_a_second_copy() {
    let gate = Arc::new(Semaphore::new(0));
    let h = harness_with(FakeExecutor {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-stuck")
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": "slow"}).to_string(),
            ))
            .unwrap()
    };

    // The winner reserves the key and parks inside the executor.
    let app = h.app.clone();
    let winner = tokio::spawn(async move { send(&app, request()).await });
    while h.exec.entered.load(AtomicOrdering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    let (status, _, body) = send(&h.app, request()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "idempotency_in_flight");
    assert_eq!(body["error"]["op"], "addNote");
    assert_eq!(
        h.exec.entered.load(AtomicOrdering::SeqCst),
        1,
        "giving up is not a licence to run the write a second time"
    );

    gate.add_permits(16);
    assert_eq!(winner.await.unwrap().0, StatusCode::OK);
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 1);
}

/// A replay cache whose database fails every statement, as the authz
/// matrix's Postgres did when one of its backends crashed (reported fix
/// `replay-down-reads-as-in-flight`).
struct DownReplayCache(ordo_fault::FaultClass);

#[async_trait::async_trait]
impl ordo_api::idempotency::ReplayCache for DownReplayCache {
    async fn claim(
        &self,
        _: &ordo_api::idempotency::ReplayKey,
        _: &ordo_api::idempotency::RequestFingerprint,
        _: ordo_api::idempotency::KeyBinding,
    ) -> ordo_api::idempotency::ReplayClaim {
        ordo_api::idempotency::ReplayClaim::Unavailable(self.0)
    }
    async fn put(
        &self,
        _: ordo_api::idempotency::ReplayKey,
        _: ordo_api::idempotency::RequestFingerprint,
        _: ordo_api::idempotency::StoredResponse,
        _: ordo_api::idempotency::KeyBinding,
    ) {
    }
    async fn abandon(&self, _: &ordo_api::idempotency::ReplayKey) {}
}

/// **A replay cache that cannot be reached is not an identical request in
/// flight.** A key sent once, while the cache's database is down or
/// restarting, answered `idempotency_in_flight` ("an identical request is
/// still running"): no such request existed. Now it answers the database's
/// own fault, 503 with `Retry-After`, and still never executes.
#[tokio::test(start_paused = true)]
async fn an_unreachable_replay_cache_is_not_an_identical_request_in_flight() {
    use ordo_fault::FaultClass;
    for (class, code) in [
        (FaultClass::Busy, "server_busy"),
        (FaultClass::Broken, "space_unavailable"),
    ] {
        let h = harness_full(
            FakeExecutor::default(),
            Arc::new(DownReplayCache(class)),
            ordo_api::LiveStreams::default(),
            paced(PAGE_SIZE, Duration::from_millis(50)),
        );
        let request = Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-once")
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": "once"}).to_string(),
            ))
            .unwrap();
        let (status, headers, body) = send(&h.app, request).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{class:?}: {body}");
        assert_eq!(error_code(&body), code, "{class:?}: {body}");
        assert_eq!(body["error"]["op"], "addNote");
        assert!(headers.get("retry-after").is_some(), "{class:?}: retryable");
        assert_eq!(
            h.exec.entered.load(AtomicOrdering::SeqCst),
            0,
            "an unenforced key never executes"
        );
    }
}

/// Z-018: a creating op's key stays bound to the payload that created the row
/// even after the stored response expires. The other direction — same key,
/// same payload — is still an ordinary retry.
#[tokio::test]
async fn a_creating_key_never_converges_a_changed_payload_after_the_response_expires() {
    let now = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
    let clock = now.clone();
    let replay = Arc::new(
        MemoryReplayCache::with_bounds(60, 64)
            .with_clock(move || clock.load(AtomicOrdering::SeqCst)),
    );
    let h = harness_with_replay(FakeExecutor::default(), replay);
    let post = |body: &str| {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-create")
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": body}).to_string(),
            ))
            .unwrap()
    };

    // The create.
    assert_eq!(send(&h.app, post("first")).await.0, StatusCode::OK);
    // Still inside the TTL: an edited replay is already a mismatch today.
    let (status, _, body) = send(&h.app, post("edited")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "idempotency_mismatch");

    // A day later, the stored response is gone. The row it created is not.
    now.store(2_000, AtomicOrdering::SeqCst);
    let (status, _, body) = send(&h.app, post("edited")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an expired response must not turn a create into a silent converge"
    );
    assert_eq!(error_code(&body), "idempotency_mismatch");
    assert_eq!(body["error"]["op"], "addNote");
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        1,
        "the edited payload never ran"
    );

    // The other direction: same key, same payload, is an honest retry and is
    // free to run again once the promise has lapsed.
    assert_eq!(send(&h.app, post("first")).await.0, StatusCode::OK);
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 2);
}

/// The complement: a non-creating op created no row, so nothing can be
/// converged onto and the binding may lapse with its response.
#[tokio::test]
async fn a_deleting_keys_binding_lapses_with_its_response() {
    let now = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
    let clock = now.clone();
    let replay = Arc::new(
        MemoryReplayCache::with_bounds(60, 64)
            .with_clock(move || clock.load(AtomicOrdering::SeqCst)),
    );
    let h = harness_with_replay(FakeExecutor::default(), replay);
    let del = |note: &str| {
        Request::builder()
            .method("DELETE")
            .uri(format!("/op/removeNote?note={note}"))
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-del-expire")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(send(&h.app, del("note_01ABC")).await.0, StatusCode::OK);
    // Inside the TTL the key is still bound to its payload.
    let (status, _, body) = send(&h.app, del("note_02DEF")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "idempotency_mismatch");

    // Past it the promise has lapsed and the key is an ordinary first attempt
    // again — there is no created row for a changed payload to converge onto.
    now.store(2_000, AtomicOrdering::SeqCst);
    let (status, headers, _) = send(&h.app, del("note_02DEF")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-ordo-idempotent-replay"), None);
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 2);
}

// ── deletes ──

#[tokio::test]
async fn delete_happy_path() {
    let h = harness();
    let (status, _, body) = send(&h.app, delete_op("removeNote", "note=note_01ABC")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["op"], "removeNote");
    assert_eq!(body["mode"], "deletes");
    assert_eq!(body["rows"], json!([{"id": "note_01ABC"}]));
}

#[tokio::test]
async fn idempotent_delete_replays_without_reexecution() {
    let h = harness();
    let request = || {
        Request::builder()
            .method("DELETE")
            .uri("/op/removeNote?note=note_01ABC")
            .header("authorization", "Bearer tok-ok")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", "k-del-1")
            .body(Body::empty())
            .unwrap()
    };
    let (status1, headers1, body1) = send(&h.app, request()).await;
    let (status2, headers2, body2) = send(&h.app, request()).await;
    assert_eq!(status1, StatusCode::OK);
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(body1, body2);
    assert!(headers1.get("x-ordo-idempotent-replay").is_none());
    assert_eq!(headers2.get("x-ordo-idempotent-replay").unwrap(), "true");
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        1,
        "executor ran exactly once"
    );
}

// ── live (SSE) ──

async fn read_until(
    stream: &mut (impl futures::Stream<Item = Result<Bytes, axum::Error>> + Unpin),
    buffer: &mut String,
    needle: &str,
) {
    while !buffer.contains(needle) {
        let chunk = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("frame within timeout")
            .expect("stream still open")
            .expect("chunk ok");
        buffer.push_str(std::str::from_utf8(&chunk).unwrap());
    }
}

use axum::body::Bytes;

#[tokio::test]
async fn live_sse_initial_then_invalidate() {
    let h = harness();
    let request = Request::builder()
        .method("GET")
        .uri("/op/vendorSpend?live=1&from=2026-01-01")
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    assert_eq!(
        response.headers().get("x-ordo-op-version").unwrap(),
        "vs-v1"
    );

    let mut stream = response.into_body().into_data_stream();
    let mut buffer = String::new();

    // Initial frame: a default `data:` event with the ordinary envelope.
    read_until(&mut stream, &mut buffer, "\n\n").await;
    assert!(
        buffer.starts_with("data:"),
        "initial frame is a plain data event: {buffer}"
    );
    assert!(buffer.contains("Acme"));
    assert!(!buffer.contains("invalidate"));

    // A bus signal becomes an `invalidate` event — no data pushed.
    h.bus.publish("vendorSpend");
    read_until(&mut stream, &mut buffer, "event: invalidate").await;
    assert!(buffer.contains("event: invalidate"));
}

#[tokio::test]
async fn live_flag_rejected_when_not_capable() {
    let h = harness();
    // ?live=1 on a read op that is not live-capable — the one remaining
    // `not_live` case now that one-shot GET is legal.
    let (status, _, body) = send(&h.app, get_op("auditTrail", "live=1")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "not_live");

    // The same op without the flag is an ordinary one-shot read.
    let (status, _, body) = send(&h.app, get_op("auditTrail", "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"][0]["what"], "space created");
}

// ── version skew headers ──

#[tokio::test]
async fn version_skew_header_flags_stale_pin() {
    let h = harness();
    let request = Request::builder()
        .method("GET")
        .uri("/op/vendorSpend?from=2026-01-01")
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .header("x-ordo-version", "vs-v0")
        .body(Body::empty())
        .unwrap();
    let (status, headers, _) = send(&h.app, request).await;
    // Soft signal: the call still executes.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-ordo-op-version").unwrap(), "vs-v1");
    assert_eq!(headers.get("x-ordo-version-skew").unwrap(), "stale");
}

// ── hot swap ──

#[tokio::test]
async fn registry_swap_leaves_in_flight_requests_on_old_version() {
    let gate = Arc::new(Semaphore::new(0));
    let h = harness_with(FakeExecutor {
        gate: Some(gate.clone()),
        ..Default::default()
    });

    // Hold a request in flight inside the executor.
    let app = h.app.clone();
    let in_flight =
        tokio::spawn(async move { send(&app, get_op("vendorSpend", "from=2026-01-01")).await });
    // Wait until the request has cloned its spec and is parked on the gate.
    timeout(Duration::from_secs(2), async {
        while h.exec.entered.load(AtomicOrdering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("request reached the executor");
    assert!(!in_flight.is_finished());

    // Swap the registry to v2 while the request is in flight, then release.
    h.registry.swap(registry_with("vs-v2"));
    gate.add_permits(16);

    let (status, headers, body) = in_flight.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["version"], "vs-v1",
        "in-flight request finished on its dispatched version"
    );
    assert_eq!(headers.get("x-ordo-op-version").unwrap(), "vs-v1");

    // The next request sees the swapped registry.
    let (_, _, body) = send(&h.app, get_op("vendorSpend", "from=2026-01-01")).await;
    assert_eq!(body["version"], "vs-v2");
}

// ── space binding: the dimension every access decision reads (SH-19 / Y-034) ──

#[tokio::test]
async fn missing_space_is_a_typed_rejection_never_a_default() {
    let h = harness();
    // Neither header nor envelope names a space. There is no "the caller's
    // space" to fall back on — a human belongs to many.
    let request = Request::builder()
        .method("GET")
        .uri("/op/vendorSpend?from=2026-01-01")
        .header("authorization", "Bearer tok-ok")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "missing_space");
    assert_eq!(body["error"]["param"], "$space");

    // Same on the POST transport, where `$space` rides the body.
    let request = Request::builder()
        .method("POST")
        .uri("/op/addNote")
        .header("authorization", "Bearer tok-ok")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"txn": "txn_01J8ZK", "body": "hi"}).to_string(),
        ))
        .unwrap();
    let (status, _, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "missing_space");

    // An empty or ill-formed id is the same answer — not a distinct code that
    // would tell a caller which spaces are well-formed.
    let (_, _, body) = send(
        &h.app,
        get_op(
            "vendorSpend",
            &format!("from=2026-01-01&$space={}", urlenc("spc 01/../..")),
        ),
    )
    .await;
    assert_eq!(error_code(&body), "missing_space");
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn header_and_envelope_space_disagreement_is_missing_space() {
    let h = harness();
    // A request that names two spaces is a request that names none: the
    // engine picks neither, because either choice is the wrong one for
    // someone. (Header wins would let a proxy override the body; body wins
    // would let a caller override the header the SDK sets.)
    let request = Request::builder()
        .method("POST")
        .uri("/op/addNote")
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"$space": "spc_victim", "txn": "txn_01J8ZK", "body": "hi"}).to_string(),
        ))
        .unwrap();
    let (status, _, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "missing_space");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("different spaces"));
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 0);

    // Agreeing on the same space through both channels is fine.
    let request = Request::builder()
        .method("POST")
        .uri("/op/addNote")
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"$space": SPACE, "txn": "txn_01J8ZK", "body": "hi"}).to_string(),
        ))
        .unwrap();
    let (status, _, _) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn calling_another_space_is_denied_at_the_door() {
    let h = harness();
    // A valid token for a real op — but naming a space the principal has no
    // membership in. Space ids leak (invitations, URLs, support tickets), so
    // this is the request an attacker actually sends.
    let request = Request::builder()
        .method("GET")
        .uri("/op/vendorSpend?from=2026-01-01")
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", "spc_victim")
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");
    // A space the principal cannot reach answers exactly like an op that does
    // not exist: no version headers, nothing that confirms either.
    assert!(headers.get("x-ordo-op-version").is_none());
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        0,
        "denied before any execution"
    );
}

/// `plan-identity-service.md` 4.3: a deployment person whose touch is too
/// old is answered `403 presence_required`, nothing runs, and every other
/// denied token keeps the uniform `access_denied`.
#[tokio::test]
async fn a_stale_touch_is_presence_required_and_nothing_runs() {
    let h = harness();
    let call = |token: &str, key: &str| {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", key.to_string())
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": "hi"}).to_string(),
            ))
            .unwrap()
    };
    let (status, _, body) = send(&h.app, call("tok-stale", "k-stale-1")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(error_code(&body), "presence_required");
    let (status, _, body) = send(&h.app, call("tok-nobody", "k-stale-2")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(error_code(&body), "access_denied");
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 0, "nothing ran");
}

/// Owner ruling 2026-10-22: the app developer may require a verified email
/// address. A person without one is refused `403 email_required` for an op
/// it covers, nothing runs, and the message names the setting. Once they
/// have one, the same call runs.
#[tokio::test]
async fn an_op_that_needs_an_email_address_is_email_required_until_the_person_adds_one() {
    let h = harness();
    let call = |key: &str| {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", "Bearer tok-mail")
            .header("content-type", "application/json")
            .header("x-ordo-space", SPACE)
            .header("idempotency-key", key.to_string())
            .body(Body::from(
                json!({"txn": "txn_01J8ZK", "body": "hi"}).to_string(),
            ))
            .unwrap()
    };
    let (status, _, body) = send(&h.app, call("k-mail-1")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(error_code(&body), "email_required");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ORDO_EMAIL_REQUIRED"),
        "the refusal names the setting: {body}"
    );
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 0, "nothing ran");

    MAIL_ADDED.store(true, AtomicOrdering::SeqCst);
    let (status, _, body) = send(&h.app, call("k-mail-2")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 1);
}

/// Owner, 2026-10-21: a call into a person's own tenant before it is set up
/// is a retryable `tenant_not_ready`, 503 with `Retry-After`, and nothing
/// runs. Anyone else calling that tenant gets the uniform denial.
#[tokio::test]
async fn a_call_into_a_tenant_still_being_set_up_is_tenant_not_ready_for_its_person_only() {
    let h = harness();
    let call = |token: &str| {
        Request::builder()
            .method("POST")
            .uri("/op/addNote")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("x-ordo-space", NOT_READY_SPACE)
            .header("idempotency-key", "k-not-ready")
            .body(Body::from(json!({"txn": "t1", "body": "hi"}).to_string()))
            .unwrap()
    };
    let (status, headers, body) = send(&h.app, call("tok-two")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(error_code(&body), "tenant_not_ready");
    assert_eq!(headers.get("retry-after").unwrap(), "7");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("/auth/provisioning"),
        "{body}"
    );

    let (status, headers, body) = send(&h.app, call("tok-ok")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");
    assert!(headers.get("retry-after").is_none());
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        0,
        "nothing ran for either caller"
    );
}

// ── the space reaches the executor (SH2-1 / Y-015) ──

#[tokio::test]
async fn executor_receives_the_space_the_door_bound() {
    let h = harness();
    let (status, _, body) = send(&h.app, get_op("spaceEcho", "")).await;
    assert_eq!(status, StatusCode::OK);
    // The executor planned against the door's space, not one it re-derived.
    assert_eq!(body["rows"][0]["space"], SPACE);
    assert_eq!(h.exec.last_space.lock().unwrap().as_deref(), Some(SPACE));
}

#[tokio::test]
async fn caller_supplied_space_cannot_steer_execution() {
    let h = harness();
    // The SH2-1 attack: satisfy the access check for one space, then name
    // another in an ordinary parameter and hope the executor reads that.
    let (status, _, body) = send(&h.app, get_op("spaceEcho", "claim=spc_victim")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error_code(&body), "internal");
    // The door's space is still the only one that reached the executor.
    assert_eq!(h.exec.last_space.lock().unwrap().as_deref(), Some(SPACE));

    // A parameter that agrees with the door executes normally.
    let (status, _, body) = send(&h.app, get_op("spaceEcho", &format!("claim={SPACE}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"][0]["space"], SPACE);
}

#[test]
fn an_op_may_not_declare_an_envelope_key_as_a_parameter() {
    // `$space` must be unshadowable: registration refuses the spec rather
    // than letting a second, caller-controlled space ride the parameter map.
    let mut shadow = space_echo_spec();
    shadow.params[0].name = "$space".into();
    assert_eq!(
        MapRegistry::new([shadow]).err(),
        Some(SpecError::ReservedParamName {
            op: "spaceEcho".into(),
            param: "$space".into(),
        })
    );
}

// ── sealed-field refinement rules (SH-4 / Y-033) ──

/// `$refine` as a URL-encoded query value, for the sealed-rule cases.
fn refine_query(op_params: &str, refine: Value) -> String {
    let encoded = urlenc(&refine.to_string());
    if op_params.is_empty() {
        format!("$refine={encoded}")
    } else {
        format!("{op_params}&$refine={encoded}")
    }
}

#[tokio::test]
async fn sealed_field_without_opt_in_is_not_refinable() {
    let h = harness();
    let refine = json!({"where": [{"field": "bankAccount", "op": "eq", "value": "GB00 1234"}]});
    let (status, _, body) = send(
        &h.app,
        get_op("vendorSpend", &refine_query("from=2026-01-01", refine)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_refinement");
    assert_eq!(body["error"]["param"], "bankAccount");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not refinable"));
    assert_eq!(h.exec.calls.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn opted_in_sealed_field_accepts_equality_only() {
    let h = harness();
    // Equality rides the blind index: allowed.
    let ok = json!({"where": [{"field": "taxId", "op": "eq", "value": "GB-42"}]});
    let (status, _, _) = send(
        &h.app,
        get_op("vendorSpend", &refine_query("from=2026-01-01", ok)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Ordering comparisons are the oracle rule 2 closes — one refusal per
    // operator, all of them typed and naming the field.
    for op in ["gt", "lt", "ge", "le", "ne"] {
        let refine = json!({"where": [{"field": "taxId", "op": op, "value": "GB-42"}]});
        let (status, _, body) = send(
            &h.app,
            get_op("vendorSpend", &refine_query("from=2026-01-01", refine)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "operator {op}");
        assert_eq!(error_code(&body), "invalid_refinement");
        assert_eq!(body["error"]["param"], "taxId");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("equality only"));
    }
}

#[tokio::test]
async fn aggregate_only_op_refuses_every_sealed_condition() {
    let h = harness();
    // Open conditions on a count are ordinary and still work.
    let open = json!({"where": [{"field": "month", "op": "eq", "value": "2026-01"}]});
    let (status, _, body) = send(&h.app, get_op("vendorCount", &refine_query("", open))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"][0]["n"], 4);

    // The sealed field the author *did* open to refinement is still refused
    // here: if you may not read the rows, you may not measure them.
    let sealed = json!({"where": [{"field": "taxId", "op": "eq", "value": "GB-42"}]});
    let (status, _, body) = send(&h.app, get_op("vendorCount", &refine_query("", sealed))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_refinement");
    assert_eq!(body["error"]["param"], "taxId");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("aggregate-only"));
    assert_eq!(
        h.exec.calls.load(AtomicOrdering::SeqCst),
        1,
        "only the open count executed"
    );
}

// ── the effective cost class per request (§7.1 / §2.1 rule 4) ──

/// **The escalation, refused.** A caller refines a reduce-shaped op on a
/// sealed field that no blind index answers. Registration proved the op
/// returns one row and asked for no acknowledgment; the condition applies
/// before the reduce, so serving it means opening every sealed payload in
/// the space — `unbounded-host` work under an op nobody registered as
/// unbounded.
///
/// The assertions that matter beyond the status code: the class is
/// `refinement_unbounded` rather than a generic 400, the field that did it
/// is named, and **the executor is never entered**. A gate that refuses
/// after the work has started is not a cost gate.
#[tokio::test]
async fn a_sealed_refinement_may_not_raise_an_op_past_its_registered_class() {
    let h = harness();
    let refine = json!({"where": [{"field": "note", "op": "eq", "value": "gift"}]});
    let (status, _, body) = send(&h.app, get_op("vendorTotal", &refine_query("", refine))).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "refinement_unbounded");
    assert_eq!(body["error"]["param"], "note");
    assert_eq!(body["error"]["op"], "vendorTotal");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("unbounded-host") && message.contains("unbounded` acknowledgment"),
        "the message must say what the class became and what was missing: {message}"
    );
    assert_eq!(
        h.exec.entered.load(AtomicOrdering::SeqCst),
        0,
        "nothing may execute: the whole point is that the work never starts"
    );
}

/// The same refinement on the op that **did** acknowledge. The `unbounded`
/// marker is a human's sentence — *this op's work grows with the size of the
/// space, and I mean it* — and §7.1 rejects only the op that became unbounded
/// quietly, so the cost gate passes it.
///
/// The venue then answers: host-side filtering is not built, so the door
/// refuses the condition as `sealed_unsupported`, naming the field (ruled
/// 2026-10-05, `refinement-default` ask 3). The class is what the
/// acknowledgment changed; the executor is still never entered.
#[tokio::test]
async fn the_acknowledgment_passes_the_cost_gate_and_the_venue_answers() {
    let h = harness();
    let refine = json!({"where": [{"field": "note", "op": "eq", "value": "gift"}]});
    let (status, _, body) = send(
        &h.app,
        get_op("vendorTotalAcked", &refine_query("", refine)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(error_code(&body), "sealed_unsupported");
    assert_eq!(body["error"]["param"], "note");
    assert_eq!(h.exec.entered.load(AtomicOrdering::SeqCst), 0);
}

/// **The other half of the proof.** A gate that refuses the escalation and
/// also refuses ordinary refinements has broken every refining caller, and
/// the failure would look like a security feature working. So: the same
/// sealed field on a row-shaped op (§7.1's post-take carve-out — the
/// candidate count is the page, not the entity), an open condition on the
/// very op whose candidate set is unbounded, and the blind-index equality
/// that rides the digest. All three are served.
#[tokio::test]
async fn ordinary_refinements_still_pass_the_cost_gate() {
    let h = harness();

    // Row-shaped: refinement is post-take, so the venue rises and the
    // cardinality does not. `bounded-host` is not a class the COST gate
    // refuses; the venue does, naming the field, because host-side
    // filtering is not built (ruled 2026-10-05).
    let sealed = json!({"where": [{"field": "note", "op": "eq", "value": "gift"}]});
    let (status, _, body) = send(&h.app, get_op("vendorPage", &refine_query("", sealed))).await;
    assert_ne!(error_code(&body), "refinement_unbounded");
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(error_code(&body), "sealed_unsupported");
    assert_eq!(body["error"]["param"], "note");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("sealed indexed refinable"));

    // An OPEN condition on the unbounded-candidate op. The candidate set is
    // every row either way; an open predicate pushes into the same statement
    // and costs the venue nothing, so `sql ⊔ sql = sql`.
    let open = json!({"where": [{"field": "month", "op": "eq", "value": "2026-01"}]});
    let (status, _, body) = send(&h.app, get_op("vendorTotal", &refine_query("", open))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an open refinement is not host work"
    );
    assert_eq!(body["rows"][0]["n"], 4);

    // `pick` names fields too, and narrowing to the sealed one is not a
    // condition: it removes columns from a result the op already produces.
    let picked = json!({"pick": ["note"]});
    let (status, _, _) = send(&h.app, get_op("vendorTotal", &refine_query("", picked))).await;
    assert_eq!(status, StatusCode::OK, "a pick adds no work");

    // And the documented sealed shape — `sealed indexed refinable`, equality
    // riding the blind index — stays a `blind-index` request, which is not
    // `unbounded-host` and is not refused.
    let indexed = json!({"where": [{"field": "taxId", "op": "eq", "value": "GB-42"}]});
    let (status, _, _) = send(
        &h.app,
        get_op("vendorSpend", &refine_query("from=2026-01-01", indexed)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// The two halves, in the order the door runs them. §7.1: "implementing only
/// one half leaves the hole open in one direction". With the aggregate flag
/// set, the *declaration-time* rule answers first and the request never
/// reaches the cost gate; with it unset — the borrowed-manifest producer that
/// does not set it — the cost gate is the only thing standing there. Both
/// refusals are typed, and they are not the same refusal.
#[tokio::test]
async fn both_gates_are_on_duty_and_neither_is_the_other() {
    let h = harness();
    let refine = json!({"where": [{"field": "note", "op": "eq", "value": "gift"}]});

    // Rule 4 alone (aggregate_only unset on `vendorTotal`).
    let (status, _, body) = send(
        &h.app,
        get_op("vendorTotal", &refine_query("", refine.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "refinement_unbounded");

    // Rule 3 first, when the producer does set the flag: *if you may not
    // read the rows, you may not measure them* is a disclosure rule, and it
    // refuses conditions the cost gate would happily price.
    let mut aggregate = vendor_total_spec();
    aggregate.aggregate_only = true;
    h.registry.swap(Arc::new(
        MapRegistry::new([aggregate]).expect("well-formed"),
    ));
    let (status, _, body) = send(&h.app, get_op("vendorTotal", &refine_query("", refine))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error_code(&body),
        "invalid_refinement",
        "the declaration-time gate answers first, and says something different"
    );
}

/// `refinement_unbounded` is the **door's** word, like `access_denied` is.
/// An executor that said it would be claiming the binder's authority over a
/// decision made before it was reached, so the envelope downgrades it —
/// which is what keeps "the request was refused" and "the executor is
/// broken" distinguishable.
#[test]
fn the_executor_may_not_raise_the_cost_gates_refusal() {
    assert!(!ErrorCode::RefinementUnbounded.executor_raisable());
    assert_eq!(
        ErrorCode::RefinementUnbounded.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn page_token_rides_a_header_too() {
    let h = harness();
    let (_, _, first) = send(&h.app, get_op("vendorSpend", "from=2026-01-01")).await;
    let token = first["next"].as_str().expect("next token").to_string();

    // Cursors are data: the header form keeps them out of query strings, and
    // resumes exactly as the query form does.
    let request = Request::builder()
        .method("GET")
        .uri("/op/vendorSpend?from=2026-01-01")
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .header("x-ordo-page", &token)
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"][0]["name"], "Initech");

    // Header and query naming different tokens is a typed refusal, not a
    // silent preference for one channel.
    let other = ordo_api::page::encode(
        &h.page_key,
        SPACE,
        &vendor_spend_spec("vs-v1"),
        "usr_ok",
        &PageCursor {
            keys: vec![json!("v_1")],
        },
    );
    let request = Request::builder()
        .method("GET")
        .uri(format!("/op/vendorSpend?from=2026-01-01&$page={other}"))
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .header("x-ordo-page", &token)
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_cursor");
}

// ── ?live=1&sync=1: the replica feed (plan-replica-feed.md, C4b) ──

fn vendor(id: &str, name: &str) -> Value {
    json!({"id": id, "name": name, "billed": "100.00", "month": "2026-01"})
}

fn five_vendors() -> FakeExecutor {
    let exec = FakeExecutor::default();
    *exec.sync_rows.lock().unwrap() = vec![
        vendor("v_1", "Acme"),
        vendor("v_2", "Globex"),
        vendor("v_3", "Initech"),
        vendor("v_4", "Umbrella"),
        vendor("v_5", "Hooli"),
    ];
    exec
}

fn sync_request(name: &str) -> Request<Body> {
    get_op(name, "live=1&sync=1")
}

/// A reconnect that says what the device already holds (C4d).
fn resume_request(name: &str, hash: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/op/{name}?live=1&sync=1"))
        .header("authorization", "Bearer tok-ok")
        .header("x-ordo-space", SPACE)
        .header("x-ordo-resume", hash)
        .body(Body::empty())
        .unwrap()
}

/// The `result` hash off a stream's completing load frame.
fn result_hash_of(buffer: &str) -> String {
    frames(buffer)
        .into_iter()
        .filter(|(event, _)| event == "load")
        .find_map(|(_, data)| data["result"].as_str().map(str::to_string))
        .expect("a completing load frame carries `result`")
}

/// Every complete frame in `buffer`, as (event, data). A frame without an
/// `event:` line is the default `message`; keep-alive comments carry no data
/// and are skipped.
fn frames(buffer: &str) -> Vec<(String, Value)> {
    buffer
        .split("\n\n")
        .filter_map(|frame| {
            let mut event = "message".to_string();
            let mut data = None;
            for line in frame.lines() {
                if let Some(e) = line.strip_prefix("event:") {
                    event = e.trim().to_string();
                } else if let Some(d) = line.strip_prefix("data:") {
                    data = serde_json::from_str::<Value>(d.trim()).ok();
                }
            }
            data.map(|d| (event, d))
        })
        .collect()
}

fn of(buffer: &str, event: &str) -> Vec<Value> {
    frames(buffer)
        .into_iter()
        .filter(|(e, _)| e == event)
        .map(|(_, d)| d)
        .collect()
}

async fn until(
    stream: &mut (impl futures::Stream<Item = Result<Bytes, axum::Error>> + Unpin),
    buffer: &mut String,
    done: impl Fn(&str) -> bool,
) {
    while !done(buffer) {
        let chunk = timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("frame within timeout")
            .expect("stream still open")
            .expect("chunk ok");
        buffer.push_str(std::str::from_utf8(&chunk).unwrap());
    }
}

fn loaded(buffer: &str) -> bool {
    of(buffer, "load").iter().any(|d| d["complete"] == true)
}

/// `open_sync` for a request the caller built — a resume carries a header.
async fn open_stream(
    h: &Harness,
    request: Request<Body>,
) -> impl futures::Stream<Item = Result<Bytes, axum::Error>> + Unpin {
    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().into_data_stream()
}

/// Drain one sync to its completing frame and hand back the buffer.
async fn sync_to_completion(h: &Harness, request: Request<Body>) -> String {
    let mut stream = open_stream(h, request).await;
    let mut buffer = String::new();
    until(&mut stream, &mut buffer, loaded).await;
    buffer
}

async fn open_sync(h: &Harness) -> impl futures::Stream<Item = Result<Bytes, axum::Error>> + Unpin {
    let response = h
        .app
        .clone()
        .oneshot(sync_request("syncRows"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().into_data_stream()
}

#[tokio::test]
async fn a_sync_loads_in_pages_then_sends_only_what_changed() {
    let h = harness_with(five_vendors());
    let mut stream = open_sync(&h).await;
    let mut buffer = String::new();
    until(&mut stream, &mut buffer, loaded).await;

    // Five rows, two to a page: three frames, each saying how far along the
    // load is, and only the last saying it is complete.
    let loads = of(&buffer, "load");
    let counts: Vec<u64> = loads
        .iter()
        .map(|d| d["loaded"].as_u64().unwrap())
        .collect();
    assert_eq!(counts, vec![2, 4, 5]);
    let complete: Vec<bool> = loads.iter().map(|d| d["complete"] == true).collect();
    assert_eq!(complete, vec![false, false, true]);
    assert_eq!(loads[0]["entity"], "vendors");
    assert_eq!(loads[0]["key"], "id");
    // Each page at its own position.
    let positions: Vec<u64> = loads
        .iter()
        .map(|d| d["position"].as_u64().unwrap())
        .collect();
    assert_eq!(positions, vec![1, 2, 3]);

    // An update, a delete and an insert, then one signal.
    {
        let mut rows = h.exec.sync_rows.lock().unwrap();
        rows[1] = vendor("v_2", "Globex Corp");
        rows.remove(2);
        rows.push(vendor("v_6", "Stark"));
    }
    h.bus.publish("syncRows");
    until(&mut stream, &mut buffer, |b| !of(b, "change").is_empty()).await;
    let change = &of(&buffer, "change")[0];
    assert_eq!(
        change["upserts"],
        json!([vendor("v_2", "Globex Corp"), vendor("v_6", "Stark")])
    );
    assert_eq!(change["deletes"], json!(["v_3"]));
    assert_eq!(change["entity"], "vendors");

    // A signal that changes nothing sends nothing: the next frame is the
    // next real change, and only that.
    h.bus.publish("syncRows");
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.exec.sync_rows.lock().unwrap()[0] = vendor("v_1", "Acme Inc");
    h.bus.publish("syncRows");
    until(&mut stream, &mut buffer, |b| of(b, "change").len() >= 2).await;
    let changes = of(&buffer, "change");
    assert_eq!(changes.len(), 2, "the empty pass sent no frame");
    assert_eq!(changes[1]["upserts"], json!([vendor("v_1", "Acme Inc")]));
    assert_eq!(changes[1]["deletes"], json!([]));
}

#[tokio::test]
async fn a_sync_waits_between_pages() {
    let h = harness_with(five_vendors());
    let mut stream = open_sync(&h).await;
    let mut buffer = String::new();
    until(&mut stream, &mut buffer, loaded).await;
    let wait = ordo_api::SyncPacing::default().page_wait;
    let reads = h.exec.sync_reads.lock().unwrap().clone();
    assert_eq!(reads.len(), 3, "one read per page");
    for pair in reads.windows(2) {
        assert!(
            pair[1] - pair[0] >= wait,
            "pages {:?} apart, under the {wait:?} wait",
            pair[1] - pair[0]
        );
    }
}

/// `ORDO_SYNC_PAGE_ROWS` reaches the reader: frames of 7 rows, and the
/// executor is asked for 7 on every read of the load.
#[tokio::test]
async fn a_sync_sends_frames_of_the_configured_page_rows() {
    let exec = FakeExecutor::default();
    *exec.sync_rows.lock().unwrap() = (1..=20)
        .map(|i| vendor(&format!("v_{i:02}"), "Acme"))
        .collect();
    let h = harness_sync(exec, paced(7, Duration::ZERO));
    let mut stream = open_sync(&h).await;
    let mut buffer = String::new();
    until(&mut stream, &mut buffer, loaded).await;
    let sizes: Vec<usize> = of(&buffer, "load")
        .iter()
        .map(|f| f["rows"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, vec![7, 7, 6]);
    assert_eq!(*h.exec.sync_page_sizes.lock().unwrap(), vec![7, 7, 7]);
}

/// A page wait of 0 sends with no sleep between reads.
#[tokio::test]
async fn a_page_wait_of_zero_sends_with_no_sleep() {
    let exec = FakeExecutor::default();
    *exec.sync_rows.lock().unwrap() = (1..=40)
        .map(|i| vendor(&format!("v_{i:02}"), "Acme"))
        .collect();
    // 40 pages: at the default 50 ms this load would take two seconds.
    let h = harness_sync(exec, paced(1, Duration::ZERO));
    let started = std::time::Instant::now();
    let mut stream = open_sync(&h).await;
    let mut buffer = String::new();
    until(&mut stream, &mut buffer, loaded).await;
    assert_eq!(of(&buffer, "load").len(), 40);
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "40 pages took {:?} with no wait",
        started.elapsed()
    );
}

/// A change made while a load is still arriving reaches the device once the
/// load completes, even when it touched a page already sent.
#[tokio::test]
async fn a_change_during_the_load_arrives_once_it_completes() {
    let h = harness_with(five_vendors());
    let mut stream = open_sync(&h).await;
    let mut buffer = String::new();
    until(&mut stream, &mut buffer, |b| !of(b, "load").is_empty()).await;
    assert!(!loaded(&buffer), "the premise: the load is still arriving");
    h.exec.sync_rows.lock().unwrap()[0] = vendor("v_1", "Acme Inc");
    h.bus.publish("syncRows");

    until(&mut stream, &mut buffer, |b| !of(b, "change").is_empty()).await;
    let order: Vec<String> = frames(&buffer).into_iter().map(|(e, _)| e).collect();
    assert_eq!(order, vec!["load", "load", "load", "change"]);
    assert_eq!(
        of(&buffer, "change")[0]["upserts"],
        json!([vendor("v_1", "Acme Inc")])
    );
}

/// Signals that queue up are answered by one pass, not one pass each.
#[tokio::test]
async fn queued_signals_are_answered_by_one_pass() {
    let h = harness_with(five_vendors());
    let mut stream = open_sync(&h).await;
    let mut buffer = String::new();
    until(&mut stream, &mut buffer, loaded).await;
    let before = h.exec.sync_reads.lock().unwrap().len();

    h.exec.sync_rows.lock().unwrap()[4] = vendor("v_5", "Hooli XYZ");
    for _ in 0..5 {
        h.bus.publish("syncRows");
    }
    until(&mut stream, &mut buffer, |b| !of(b, "change").is_empty()).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let passes = h.exec.sync_reads.lock().unwrap().len() - before;
    assert_eq!(passes, 3, "one pass of three pages, not five");
    assert_eq!(of(&buffer, "change").len(), 1);
}

#[tokio::test]
async fn a_sync_of_an_op_without_row_identity_is_refused_naming_the_rule() {
    let h = harness();
    let (status, _, body) = send(
        &h.app,
        get_op("vendorSpend", "live=1&sync=1&from=2026-01-01"),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let text = body.to_string();
    assert!(text.contains("not_live"), "{text}");
    assert!(text.contains("no_anchor"), "the rule is named: {text}");
    // The same op still serves a plain live subscription.
    let response = h
        .app
        .clone()
        .oneshot(get_op("vendorSpend", "live=1&from=2026-01-01"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

// ── C4d: a reconnect says what it holds, and an unchanged result sends nothing ──

#[tokio::test]
async fn a_completed_load_names_the_result_it_sent() {
    let h = harness_with(five_vendors());
    let first = sync_to_completion(&h, sync_request("syncRows")).await;
    let hash = result_hash_of(&first);
    assert_eq!(hash.len(), 32, "sixteen bytes of hex");

    // Only the completing frame carries it: a hash of a prefix would resume
    // a device onto a partial result.
    let loads = of(&first, "load");
    assert!(loads[..loads.len() - 1]
        .iter()
        .all(|f| f["result"].is_null()));

    // The same rows hash the same way — it names a result, not a read.
    let again = sync_to_completion(&h, sync_request("syncRows")).await;
    assert_eq!(result_hash_of(&again), hash);
}

#[tokio::test]
async fn a_resume_on_an_unchanged_result_sends_no_rows() {
    let h = harness_with(five_vendors());
    let hash = result_hash_of(&sync_to_completion(&h, sync_request("syncRows")).await);

    let resumed = sync_to_completion(&h, resume_request("syncRows", &hash)).await;
    let loads = of(&resumed, "load");
    assert_eq!(loads.len(), 1, "one frame, not a paged load: {loads:?}");
    assert_eq!(loads[0]["rows"].as_array().unwrap().len(), 0);
    assert_eq!(loads[0]["resumed"], json!(true));
    assert_eq!(loads[0]["complete"], json!(true));
    assert_eq!(loads[0]["loaded"], json!(5), "it still says what it has");
    assert_eq!(loads[0]["result"].as_str().unwrap(), hash);
}

#[tokio::test]
async fn a_resume_after_a_change_falls_back_to_a_full_load() {
    let h = harness_with(five_vendors());
    let stale = result_hash_of(&sync_to_completion(&h, sync_request("syncRows")).await);

    h.exec.sync_rows.lock().unwrap()[0] = vendor("v_1", "Acme Inc");

    let reloaded = sync_to_completion(&h, resume_request("syncRows", &stale)).await;
    let loads = of(&reloaded, "load");
    let rows: usize = loads
        .iter()
        .map(|f| f["rows"].as_array().unwrap().len())
        .sum();
    assert_eq!(rows, 5, "the whole result, not a difference");
    assert!(
        loads.iter().all(|f| f["resumed"] != json!(true)),
        "a changed result is not a resume"
    );
    assert_ne!(
        result_hash_of(&reloaded),
        stale,
        "and it hands back the new hash"
    );
}

#[tokio::test]
async fn a_change_frame_names_the_result_after_it_so_a_resume_matches_what_the_device_holds() {
    // Notes e2e, test 7: a device that held a load's hash through later
    // changes resumed onto rows it no longer matched. Each change frame now
    // says the hash of the result after it, the same hash a fresh load of
    // that result names.
    let h = harness_with(five_vendors());
    let mut stream = open_sync(&h).await;
    let mut buffer = String::new();
    until(&mut stream, &mut buffer, loaded).await;
    let before = result_hash_of(&buffer);

    // A row leaves the result, then a row joins it, each heard on its own.
    h.exec.sync_rows.lock().unwrap().remove(0);
    h.bus.publish("syncRows");
    until(&mut stream, &mut buffer, |b| of(b, "change").len() == 1).await;
    h.exec
        .sync_rows
        .lock()
        .unwrap()
        .push(vendor("v_7", "Wayne"));
    h.bus.publish("syncRows");
    until(&mut stream, &mut buffer, |b| of(b, "change").len() == 2).await;

    let changes = of(&buffer, "change");
    assert_eq!(changes[0]["deletes"], json!(["v_1"]));
    assert_eq!(changes[1]["upserts"], json!([vendor("v_7", "Wayne")]));
    let after_first = changes[0]["result"]
        .as_str()
        .expect("a change names its result");
    let after_second = changes[1]["result"]
        .as_str()
        .expect("a change names its result");
    assert_ne!(after_first, before);
    assert_ne!(after_second, after_first);

    // A fresh load of the same rows names the same hash, so the device's
    // next connection resumes on exactly what it holds.
    let fresh = sync_to_completion(&h, sync_request("syncRows")).await;
    assert_eq!(result_hash_of(&fresh), after_second);
    let resumed = sync_to_completion(&h, resume_request("syncRows", after_second)).await;
    assert_eq!(of(&resumed, "load")[0]["resumed"], json!(true));
}

/// Notes e2e, Firefox (2026-10-20): a second tab's feed stayed pending. The
/// space rides in a header, so two feeds of one op share a URL, and Firefox
/// made the second wait on the HTTP cache entry the first, endless, stream
/// held. Nothing a member is answered may be stored by the browser.
#[tokio::test]
async fn member_answers_and_streams_are_never_stored_by_the_browser_cache() {
    let cache_control = |response: &axum::response::Response| {
        response
            .headers()
            .get("cache-control")
            .map(|v| v.to_str().unwrap().to_string())
    };
    let h = harness_with(five_vendors());

    let sync = h
        .app
        .clone()
        .oneshot(sync_request("syncRows"))
        .await
        .unwrap();
    assert_eq!(sync.status(), StatusCode::OK);
    assert_eq!(
        cache_control(&sync).as_deref(),
        Some("no-store"),
        "a sync feed"
    );

    let read = h.app.clone().oneshot(get_op("syncRows", "")).await.unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(
        cache_control(&read).as_deref(),
        Some("no-store"),
        "a one-shot read"
    );

    let shared = Request::builder()
        .method("GET")
        .uri("/live")
        .header("authorization", "Bearer tok-ok")
        .body(Body::empty())
        .unwrap();
    let shared = h.app.clone().oneshot(shared).await.unwrap();
    assert_eq!(shared.status(), StatusCode::OK);
    assert_eq!(
        cache_control(&shared).as_deref(),
        Some("no-store"),
        "the shared live stream"
    );
}

#[tokio::test]
async fn a_hash_the_server_never_minted_loads_in_full() {
    // Nothing in a resume is trusted as content: an unknown hash is a reload,
    // never an error and never a partial answer.
    let h = harness_with(five_vendors());
    let loaded_buf = sync_to_completion(
        &h,
        resume_request("syncRows", "00000000000000000000000000000000"),
    )
    .await;
    let rows: usize = of(&loaded_buf, "load")
        .iter()
        .map(|f| f["rows"].as_array().unwrap().len())
        .sum();
    assert_eq!(rows, 5);
}

// ── GET /live: one stream, many reads (27s) ──

/// Open a shared stream as `token`; hand back its frames, its id, and
/// whatever arrived after the `ready` frame.
async fn open_shared(
    h: &Harness,
    token: &str,
) -> (
    impl futures::Stream<Item = Result<Bytes, axum::Error>> + Unpin,
    String,
    String,
) {
    let request = Request::builder()
        .method("GET")
        .uri("/live")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut buffer = String::new();
    read_until(&mut stream, &mut buffer, "\n\n").await;
    assert!(buffer.starts_with("event: ready"), "{buffer}");
    let data = buffer
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .expect("ready carries data");
    let id = serde_json::from_str::<Value>(data).unwrap()["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let rest = buffer.split_once("\n\n").unwrap().1.to_string();
    (stream, id, rest)
}

fn add_read(stream: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/live/{stream}"))
        .header("authorization", format!("Bearer {token}"))
        .header("x-ordo-space", SPACE)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn drop_read(stream: &str, token: &str, key: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(format!("/live/{stream}/{key}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// The keys of every `invalidate` frame in `buffer`, in order.
fn invalidated_keys(buffer: &str) -> Vec<String> {
    buffer
        .split("\n\n")
        .filter(|f| f.contains("event: invalidate"))
        .filter_map(|f| f.lines().find_map(|l| l.strip_prefix("data: ")))
        .map(|d| {
            serde_json::from_str::<Value>(d).unwrap()["key"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

fn spend_read(key: &str) -> Value {
    json!({"key": key, "op": "vendorSpend", "params": {"from": "2026-01-01"}})
}

fn rows_read(key: &str) -> Value {
    json!({"key": key, "op": "syncRows"})
}

#[tokio::test]
async fn shared_stream_two_reads_each_get_their_own_invalidations() {
    let h = harness();
    let (mut stream, id, mut buffer) = open_shared(&h, "tok-ok").await;

    let (status, headers, body) = send(&h.app, add_read(&id, "tok-ok", spend_read("a"))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The answer is the read's first envelope, with the read's version.
    assert_eq!(body["op"], "vendorSpend");
    assert_eq!(headers.get("x-ordo-op-version").unwrap(), "vs-v1");
    let (status, _, body) = send(&h.app, add_read(&id, "tok-ok", rows_read("b"))).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    h.bus.publish("vendorSpend");
    read_until(&mut stream, &mut buffer, "\"key\":\"a\"").await;
    assert_eq!(invalidated_keys(&buffer), vec!["a"]);

    h.bus.publish("syncRows");
    read_until(&mut stream, &mut buffer, "\"key\":\"b\"").await;
    assert_eq!(invalidated_keys(&buffer), vec!["a", "b"]);
}

#[tokio::test]
async fn shared_stream_a_refused_read_leaves_the_stream_open() {
    let h = harness();
    let (mut stream, id, mut buffer) = open_shared(&h, "tok-ok").await;

    // Each read meets the door alone: a forbidden op answers exactly as
    // `GET /op/ownerOnly?live=1` would, and so does an unknown one.
    let forbidden = json!({"key": "x", "op": "ownerOnly"});
    let (status, headers, body) = send(&h.app, add_read(&id, "tok-ok", forbidden)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");
    assert!(headers.get("x-ordo-op-version").is_none());
    let (alone_status, _, alone) = send(&h.app, get_op("ownerOnly", "live=1")).await;
    assert_eq!(alone_status, status);
    assert_eq!(alone, body);
    let unknown = json!({"key": "x", "op": "noSuchOp"});
    let (status, _, unknown) = send(&h.app, add_read(&id, "tok-ok", unknown)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(unknown["error"]["message"], body["error"]["message"]);

    // A read that is not live-capable, a space the caller is not in, and a
    // missing space are refused too.
    let not_live = json!({"key": "x", "op": "auditTrail"});
    let (status, _, body) = send(&h.app, add_read(&id, "tok-ok", not_live)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "not_live");
    let mut elsewhere = add_read(&id, "tok-ok", rows_read("x"));
    elsewhere
        .headers_mut()
        .insert("x-ordo-space", "spc_99".parse().unwrap());
    let (status, _, body) = send(&h.app, elsewhere).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");
    let mut no_space = add_read(&id, "tok-ok", rows_read("x"));
    no_space.headers_mut().remove("x-ordo-space");
    let (status, _, body) = send(&h.app, no_space).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "missing_space");

    // The stream is still open and still carries a read added after.
    let (status, _, _) = send(&h.app, add_read(&id, "tok-ok", rows_read("ok"))).await;
    assert_eq!(status, StatusCode::OK);
    h.bus.publish("syncRows");
    read_until(&mut stream, &mut buffer, "\"key\":\"ok\"").await;
    assert_eq!(invalidated_keys(&buffer), vec!["ok"]);
}

#[tokio::test]
async fn shared_stream_unsubscribe_stops_events() {
    let h = harness();
    let (mut stream, id, mut buffer) = open_shared(&h, "tok-ok").await;
    let (status, _, _) = send(&h.app, add_read(&id, "tok-ok", spend_read("a"))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&h.app, add_read(&id, "tok-ok", rows_read("b"))).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = send(&h.app, drop_read(&id, "tok-ok", "a")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Dropping a key the stream does not carry is the same answer.
    let (status, _, _) = send(&h.app, drop_read(&id, "tok-ok", "a")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The dropped read's bus subscription is released, so `vendorSpend`
    // reaches nobody, and only `b`'s signal arrives.
    for _ in 0..50 {
        if h.bus.publish("vendorSpend") == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(h.bus.publish("vendorSpend"), 0);
    h.bus.publish("syncRows");
    read_until(&mut stream, &mut buffer, "\"key\":\"b\"").await;
    assert_eq!(invalidated_keys(&buffer), vec!["b"]);
}

#[tokio::test]
async fn shared_stream_belongs_to_the_token_that_opened_it() {
    let h = harness();
    let (_stream, id, _) = open_shared(&h, "tok-ok").await;

    // Another token, and a stream that never existed: one answer.
    let (status, _, other) = send(&h.app, add_read(&id, "tok-two", rows_read("a"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, _, unknown) = send(&h.app, add_read("ls_nope", "tok-ok", rows_read("a"))).await;
    assert_eq!(other, unknown);
    let (status, _, _) = send(&h.app, drop_read(&id, "tok-two", "a")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // No bearer at all: the door's own answer.
    let request = Request::builder()
        .method("GET")
        .uri("/live")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");
}

/// The read cap is a setting. At a cap of one, the second key is refused
/// as `quota_exceeded` naming the cap, the stream stays open, and the key
/// it carries can still be replaced.
#[tokio::test]
async fn shared_stream_read_cap_is_a_setting() {
    let h = harness_live(ordo_api::LiveSettings {
        max_reads_per_stream: 1,
        ..Default::default()
    });
    let (mut stream, id, mut buffer) = open_shared(&h, "tok-ok").await;
    let (status, _, _) = send(&h.app, add_read(&id, "tok-ok", spend_read("a"))).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, body) = send(&h.app, add_read(&id, "tok-ok", rows_read("b"))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(error_code(&body), "quota_exceeded");
    assert!(
        body["error"]["message"].as_str().unwrap().contains("(1)"),
        "the message names the cap: {body}"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ORDO_LIVE_MAX_READS"),
        "the message names the setting (audit 31): {body}"
    );

    // Replacing the read under its own key is not one more read.
    let (status, _, _) = send(&h.app, add_read(&id, "tok-ok", rows_read("a"))).await;
    assert_eq!(status, StatusCode::OK);
    h.bus.publish("syncRows");
    read_until(&mut stream, &mut buffer, "\"key\":\"a\"").await;
    assert_eq!(invalidated_keys(&buffer), vec!["a"]);
    // And the replaced read no longer signals.
    h.bus.publish("vendorSpend");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(invalidated_keys(&buffer), vec!["a"]);
}

#[tokio::test]
async fn shared_stream_closes_when_the_client_goes() {
    let h = harness();
    let (stream, id, _) = open_shared(&h, "tok-ok").await;
    let (status, _, _) = send(&h.app, add_read(&id, "tok-ok", rows_read("a"))).await;
    assert_eq!(status, StatusCode::OK);
    drop(stream);
    // The read's bus subscription is released with the stream.
    for _ in 0..50 {
        if h.bus.publish("syncRows") == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(h.bus.publish("syncRows"), 0);
    let (status, _, _) = send(&h.app, add_read(&id, "tok-ok", rows_read("b"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(start_paused = true)]
async fn shared_stream_with_no_reads_closes_after_the_idle_window() {
    let h = harness();
    let idle = h.live.settings().idle_close;
    assert_eq!(
        idle,
        Duration::from_secs(ordo_api::live::DEFAULT_IDLE_CLOSE_SECS),
        "the shipped default applies when nothing is set"
    );
    let (mut stream, _id, _) = open_shared(&h, "tok-ok").await;
    let end = timeout(idle * 2, async { while stream.next().await.is_some() {} }).await;
    assert!(end.is_ok(), "an empty stream ends on its own");
}

/// The idle window is a setting too: a short one closes an empty stream
/// sooner, and an open stream that carries a read is not closed by it.
#[tokio::test(start_paused = true)]
async fn shared_stream_idle_window_is_a_setting() {
    let h = harness_live(ordo_api::LiveSettings {
        idle_close: Duration::from_secs(2),
        ..Default::default()
    });
    let (mut stream, _id, _) = open_shared(&h, "tok-ok").await;
    let end = timeout(Duration::from_secs(4), async {
        while stream.next().await.is_some() {}
    })
    .await;
    assert!(end.is_ok(), "the shorter window closes an empty stream");

    let (mut stream, id, mut buffer) = open_shared(&h, "tok-ok").await;
    let (status, _, _) = send(&h.app, add_read(&id, "tok-ok", rows_read("a"))).await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(h.live.len(), 1, "a stream with a read stays open");
    h.bus.publish("syncRows");
    read_until(&mut stream, &mut buffer, "\"key\":\"a\"").await;
}

#[tokio::test]
async fn shared_stream_refuses_a_malformed_read() {
    let h = harness();
    let (_stream, id, _) = open_shared(&h, "tok-ok").await;
    for body in [
        json!({"key": "a b", "op": "syncRows"}),
        json!({"op": "syncRows"}),
        json!({"key": "a", "op": "syncRows", "extra": 1}),
        json!({"key": "a", "op": "vendorSpend", "params": {"from": ["2026-01-01"]}}),
    ] {
        let (status, _, reply) = send(&h.app, add_read(&id, "tok-ok", body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {reply}");
        assert_eq!(error_code(&reply), "invalid_param");
    }
}

/// A stream holds a connection before any read is checked, so the open
/// itself asks whether the deployment issued the token. A string nobody
/// signed gets the uniform answer and holds nothing: before this, any
/// bearer string opened a stream for the whole idle window.
#[tokio::test]
async fn shared_stream_is_not_opened_for_a_token_nobody_issued() {
    let h = harness();
    let request = Request::builder()
        .method("GET")
        .uri("/live")
        .header("authorization", "Bearer tok-forged")
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");
    assert!(headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("json"));
    assert!(h.live.is_empty(), "a refused open leaves no stream behind");

    // The same body a bad token gets at the op route, so the two tell a
    // stranger nothing apart.
    let mut alone = get_op("syncRows", "live=1");
    alone
        .headers_mut()
        .insert("authorization", "Bearer tok-forged".parse().unwrap());
    let (alone_status, _, alone_body) = send(&h.app, alone).await;
    assert_eq!(alone_status, status);
    assert_eq!(alone_body["error"]["code"], body["error"]["code"]);
    assert_eq!(alone_body["error"]["message"], body["error"]["message"]);

    let (_stream, _id, _) = open_shared(&h, "tok-ok").await;
    assert_eq!(h.live.len(), 1, "an issued token opens one");
}

// ── the door's value checks (plan-refinement.md section 4) ──

async fn probe(h: &Harness, op: &str, refine: Value) -> (StatusCode, Value) {
    let (status, _, body) = send(&h.app, get_op(op, &refine_query("", refine))).await;
    (status, body)
}

fn cond(field: &str, op: &str, value: Value) -> Value {
    json!({"where": [{"field": field, "op": op, "value": value}]})
}

/// A value that does not fit its field's type is `invalid_refinement`,
/// naming the field, and never reaches the executor. Before types rode on
/// `FieldSpec`, the same value reached the binder and came back a 500.
#[tokio::test]
async fn a_value_of_the_wrong_type_is_refused_at_the_door() {
    let h = harness();
    for (field, value) in [
        ("n", json!("five")),
        ("n", json!(2.5)),
        ("t", json!(5)),
        ("d", json!("2026-13-01")),
        ("d", json!("01/02/2026")),
        // A day the month does not have: Postgres refuses the literal, and
        // that refusal came back as a 500 until the door checked the
        // calendar (R8).
        ("d", json!("2026-02-31")),
        ("d", json!("2026-02-29")),
        ("at", json!("2026-04-31T09:30:00Z")),
        // Postgres text holds no NUL: `string contains embedded null` was a
        // 500 (R8).
        ("t", json!("a\u{0}b")),
        // A money column holds 64-bit minor units and a decimal literal is
        // read into 128 bits: past that the binder cannot build the literal,
        // which was a 500 (R8).
        ("m", json!("9999999999999999999")),
        ("m", json!("99999999999999999.99")),
        ("m", json!(9_223_372_036_854_775_807i64)),
        ("r", json!("999999999999999999999999999999999999999")),
        ("at", json!("2026-10-05T09:30:00")),
        ("at", json!("2026-10-05T09:30:00+02:00")),
        ("b", json!("true")),
        ("m", json!("12,50")),
        ("r", json!("1e3")),
        ("dur", json!("90d")),
        ("dur", json!(-5)),
    ] {
        let (status, body) = probe(&h, "typedProbe", cond(field, "eq", value.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field} = {value}: {body}");
        assert_eq!(error_code(&body), "invalid_refinement", "{field} = {value}");
        assert_eq!(body["error"]["param"], field);
    }
    // One mistyped member of an `in` is enough.
    let (status, body) = probe(&h, "typedProbe", cond("n", "in", json!([1, "two"]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["param"], "n");
    let (status, body) = probe(&h, "typedProbe", cond("t", "in", json!(["ok", "a\u{0}b"]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["param"], "t");
    assert_eq!(h.exec.entered.load(AtomicOrdering::SeqCst), 0);
}

/// A `pick` that names one field twice is refused by name. The projection
/// would carry the column twice, and the statement failed on the ambiguous
/// name, which came back as a 500 (R8).
#[tokio::test]
async fn a_pick_that_repeats_a_field_is_refused_at_the_door() {
    let h = harness();
    let (status, body) = probe(&h, "typedProbe", json!({"pick": ["n", "t", "n"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "invalid_refinement");
    assert_eq!(body["error"]["param"], "n");
    assert_eq!(body["error"]["message"], "`pick` names `n` twice");
    assert_eq!(h.exec.entered.load(AtomicOrdering::SeqCst), 0);
    let (status, body) = probe(&h, "typedProbe", json!({"pick": ["n", "t"]})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Money and decimal take a decimal string or an integer, never a float, as
/// the device already enforces. A value finer than the field's places is a
/// legal comparison (`m <= 2.505`), unlike a parameter, which must fit.
#[tokio::test]
async fn money_takes_a_decimal_string_or_an_integer_and_never_a_float() {
    let h = harness();
    let (status, body) = probe(&h, "typedProbe", cond("m", "eq", json!(2.5))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("float"));

    for value in [
        json!("2.50"),
        json!("-0.00"),
        json!(3),
        json!("2.505"),
        // 18 digits of minor units, the most a money(2) holds; zeros that
        // carry no value do not count.
        json!("9999999999999999.99"),
        json!("-9999999999999999.99"),
        json!("00000000000000000000000001.100000000000000000000000"),
    ] {
        let (status, body) = probe(&h, "typedProbe", cond("m", "le", value.clone())).await;
        assert_eq!(status, StatusCode::OK, "{value}: {body}");
    }
    let (status, body) = probe(
        &h,
        "typedProbe",
        cond("r", "in", json!(["0.0825", 12, null])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Every string in a refinement is normalized to NFC at the door, as a
/// parameter is, so a decomposed value matches the composed row.
#[tokio::test]
async fn refinement_text_reaches_the_executor_in_nfc() {
    let h = harness();
    let refine = json!({"where": [
        {"field": "t", "op": "eq", "value": "e\u{0301}"},
        {"field": "t", "op": "in", "value": ["e\u{0301}", "apple"]},
    ]});
    let (status, body) = probe(&h, "typedProbe", refine).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let seen = &body["rows"][0]["refine"]["where"];
    assert_eq!(seen[0]["value"], "\u{e9}");
    assert_eq!(seen[1]["value"][0], "\u{e9}");
    assert_eq!(seen[1]["value"][1], "apple");
}

/// A blob has no order and no meaningful equality: no condition names it.
/// It can still be picked.
#[tokio::test]
async fn a_blob_takes_no_condition_and_can_be_picked() {
    let h = harness();
    let (status, body) = probe(&h, "typedProbe", cond("f", "eq", json!("x"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["param"], "f");
    let (status, body) = probe(&h, "typedProbe", json!({"where": [], "pick": ["f"]})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The size limits (ruled 2026-10-05, `refinement-default` ask 6): at most
/// 16 conditions and 200 values in one `in` by default. The refusal names
/// the limit and the setting, so it is never a debugging mission.
#[tokio::test]
async fn a_refinement_past_the_size_limits_names_the_limit_and_the_setting() {
    let h = harness();
    let at = |n: usize| -> Value {
        json!({"where": (0..n).map(|i| json!({"field": "n", "op": "ne", "value": i})).collect::<Vec<_>>()})
    };
    let (status, body) = probe(&h, "typedProbe", at(16)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "16 conditions is the limit, not past it: {body}"
    );
    let (status, body) = probe(&h, "typedProbe", at(17)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "invalid_refinement");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("17")
            && message.contains("16")
            && message.contains("ORDO_REFINE_MAX_CONDITIONS"),
        "{message}"
    );

    let values = |n: usize| cond("n", "in", json!((0..n as i64).collect::<Vec<_>>()));
    let (status, body) = probe(&h, "typedProbe", values(200)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = probe(&h, "typedProbe", values(201)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["param"], "n");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("201")
            && message.contains("200")
            && message.contains("ORDO_REFINE_MAX_IN_VALUES"),
        "{message}"
    );
}

/// The limits are the deployment's: a door built with others enforces those.
#[tokio::test]
async fn the_size_limits_are_settings() {
    assert_eq!(
        ordo_api::RefineLimits::parse(None, None),
        ordo_api::RefineLimits::default()
    );
    let limits = ordo_api::RefineLimits::parse(Some("2"), Some(" 3 "));
    assert_eq!((limits.max_conditions, limits.max_in_values), (2, 3));
    // Zero and a non-number read as unset; the host refuses its boot on them.
    assert_eq!(
        ordo_api::RefineLimits::parse(Some("0"), Some("many")),
        ordo_api::RefineLimits::default()
    );

    let registry = Arc::new(RegistryHandle::new(registry_with("vs-v1")));
    let state = ApiState {
        registry,
        executor: Arc::new(FakeExecutor::default()),
        access: Arc::new(FakeAccess),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: page_key(),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: limits,
        public: Default::default(),
    };
    let app = router(state);
    let three = json!({"where": [
        {"field": "n", "op": "ne", "value": 1},
        {"field": "n", "op": "ne", "value": 2},
        {"field": "n", "op": "ne", "value": 3},
    ]});
    let (status, _, body) = send(&app, get_op("typedProbe", &refine_query("", three))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("limit is 2"));
}

/// `std.E.count`'s pairing at the door (`std-library.md` §2.1): a condition
/// names a field of the list's manifest, and a `pick` names the count's own
/// output. Each refusal lists the fields it would have taken.
#[tokio::test]
async fn a_count_takes_conditions_on_the_lists_fields_and_picks_its_own() {
    let h = harness();
    let (status, body) = probe(&h, "probeCount", cond("t", "eq", json!("apple"))).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = probe(&h, "probeCount", cond("n", "gt", json!(1))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["param"], "n");
    assert!(body["error"]["message"].as_str().unwrap().contains(": t"));

    let (status, body) = probe(&h, "probeCount", json!({"pick": ["n"]})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = probe(&h, "probeCount", json!({"pick": ["t"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("output fields: n"));
}

// ── R6: live and sync with a refinement (`plan-refinement.md` section 7) ──

/// `?live=1` takes `$refine`: the first frame is the refined read, and
/// signals stay per op.
#[tokio::test]
async fn a_live_read_first_frame_is_refined() {
    let h = harness();
    let refine = json!({"where": [{"field": "name", "op": "eq", "value": "Globex"}]});
    let request = get_op(
        "vendorSpend",
        &refine_query("live=1&from=2026-01-01", refine),
    );
    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut buffer = String::new();
    read_until(&mut stream, &mut buffer, "\n\n").await;
    let first = &of(&buffer, "message")[0];
    assert_eq!(first["rows"].as_array().unwrap().len(), 1, "{first}");
    assert_eq!(first["rows"][0]["name"], "Globex");

    h.bus.publish("vendorSpend");
    read_until(&mut stream, &mut buffer, "event: invalidate").await;
}

/// A live read's refinement meets the one-shot path's gates: an unknown
/// field is `invalid_refinement`, and a cursor is still refused.
#[tokio::test]
async fn a_live_refinement_meets_the_door() {
    let h = harness();
    let bad = json!({"where": [{"field": "nope", "op": "eq", "value": "x"}]});
    let (status, _, body) = send(
        &h.app,
        get_op("vendorSpend", &refine_query("live=1&from=2026-01-01", bad)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_refinement");

    let (status, _, body) = send(
        &h.app,
        get_op("vendorSpend", "live=1&from=2026-01-01&$page=abc"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_cursor");
}

/// Ruled 2026-10-05 (`refinement-default` ask 2): a sync refuses `pick`,
/// by class and naming it, because the device stores whole rows by id.
#[tokio::test]
async fn a_sync_refuses_pick() {
    let h = harness_with(five_vendors());
    let pick = json!({"where": [{"field": "name", "op": "ne", "value": "x"}], "pick": ["name"]});
    let (status, _, body) = send(
        &h.app,
        get_op("syncRows", &refine_query("live=1&sync=1", pick.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_refinement");
    assert_eq!(body["error"]["param"], "pick");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("refuses its `pick`"), "{message}");

    // The same pick on the plain live stream is accepted.
    let response = h
        .app
        .clone()
        .oneshot(get_op("syncRows", &refine_query("live=1", pick)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// A refined sync: the load is the filtered rows, and a row that stops
/// matching is a delete while one that starts matching is an upsert,
/// through the same diff as any other change.
#[tokio::test]
async fn a_refined_sync_sends_rows_entering_and_leaving_as_changes() {
    let h = harness_with(five_vendors());
    let refine = json!({"where": [
        {"field": "name", "op": "in", "value": ["Acme", "Globex", "Hooli"]}
    ]});
    let mut stream = open_stream(
        &h,
        get_op("syncRows", &refine_query("live=1&sync=1", refine)),
    )
    .await;
    let mut buffer = String::new();
    until(&mut stream, &mut buffer, loaded).await;
    let ids: Vec<String> = of(&buffer, "load")
        .iter()
        .flat_map(|d| d["rows"].as_array().cloned().unwrap_or_default())
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, ["v_1", "v_2", "v_5"]);
    let last = of(&buffer, "load").last().cloned().unwrap();
    assert_eq!(last["loaded"], 3);

    // Globex is renamed out of the filter; Initech is renamed into it.
    {
        let mut rows = h.exec.sync_rows.lock().unwrap();
        rows[1] = vendor("v_2", "Globex Corp");
        rows[2] = vendor("v_3", "Acme");
    }
    h.bus.publish("syncRows");
    until(&mut stream, &mut buffer, |b| !of(b, "change").is_empty()).await;
    let change = &of(&buffer, "change")[0];
    assert_eq!(change["upserts"], json!([vendor("v_3", "Acme")]));
    assert_eq!(change["deletes"], json!(["v_2"]));
}

/// The shared stream carries a refinement per read: the `POST` takes
/// `refine`, checks it as the one-shot path does, and answers the refined
/// first envelope. A refused refinement leaves the stream open.
#[tokio::test]
async fn shared_stream_carries_a_refinement_per_read() {
    let h = harness();
    let (mut stream, id, mut buffer) = open_shared(&h, "tok-ok").await;

    let mut refined = spend_read("a");
    refined["refine"] = json!({"where": [{"field": "name", "op": "eq", "value": "Acme"}]});
    let (status, _, body) = send(&h.app, add_read(&id, "tok-ok", refined)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Acme"]);

    // The same op unrefined is another read with its own key.
    let (status, _, body) = send(&h.app, add_read(&id, "tok-ok", spend_read("b"))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 2, "one page of 2");

    let mut bad = spend_read("c");
    bad["refine"] = json!({"where": [{"field": "nope", "op": "eq", "value": "x"}]});
    let (status, _, body) = send(&h.app, add_read(&id, "tok-ok", bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid_refinement");

    // Signals stay per op: both reads of vendorSpend hear one write.
    h.bus.publish("vendorSpend");
    read_until(&mut stream, &mut buffer, "\"key\":\"b\"").await;
    read_until(&mut stream, &mut buffer, "\"key\":\"a\"").await;
    let mut keys = invalidated_keys(&buffer);
    keys.sort();
    assert_eq!(keys, ["a", "b"]);
}
