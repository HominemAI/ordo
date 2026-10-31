//! Ordo API engine — the HTTP layer that serves registered ops.
//!
//! The router is generated entirely from the artifact registry: one path
//! (`/op/{name}`), no per-op code, no language on the wire. The HTTP method
//! mirrors the op's derived kind — `GET` for reads (one-shot, or SSE live
//! with `?live=1`), `POST` for writes, `DELETE` for pure-deletes; a
//! mismatch is 405 `method_mismatch` before anything else runs. Dispatch =
//! registry lookup → method enforcement → envelope split → space bind →
//! access check → param bind → refinement validation → cursor decode →
//! execute prepared plan.
//!
//! Every call names its space: `$space` in the closed envelope key set, or
//! `X-Ordo-Space` on `GET`/`DELETE`. The space is bound before the access
//! check, passed to it, and then passed **to the executor** as its own
//! argument, because the platform's access model is per space (SH-19,
//! SH2-1) — a check that cannot see the space cannot stop a member of one
//! space from calling an op against another's data, and an executor that
//! cannot see it has to re-derive one from caller-controlled input.
//!
//! Page cursors are authenticated and principal-bound (SH-6): see
//! [`page::PageKey`].
//! Protocol details live in `docs/api-engine-design.md`.
//!
//! The crate deliberately depends on no sibling crate: `OpRegistry`,
//! `OpExecutor`, `AccessChecker`, `InvalidationBus`, and `ReplayCache` are
//! the narrow boundary the rest of the platform wires in later.

pub mod attest;
pub mod bind;
pub mod blob;
pub mod bus;
pub mod cors;
pub mod download;
pub mod error;
pub mod idempotency;
pub use ordo_schema::json;
pub mod live;
pub mod mac;
pub mod page;
pub mod public;
pub mod refine;
pub mod registry;
pub mod router;
pub mod spec;
pub mod sync;

pub use blob::{
    BlobBody, BlobLimits, BlobMeta, BlobPort, BlobRead, BlobRequest, BlobState, BlobVerb,
};
pub use bus::MemoryBus;
pub use error::{ApiError, EdgeCode, EdgeError, ErrorCode, SERVER_BUSY_MESSAGE};
pub use idempotency::{
    KeyBinding, MemoryReplayCache, ReplayCache, ReplayClaim, ReplayKey, RequestFingerprint,
    StoredResponse,
};
pub use json::JsonLimits;
pub use live::{LiveSettings, LiveStreams};
pub use page::{PageCursor, PageKey, PageKeyError};
pub use public::{
    HumanVerdict, HumanVerifier, PublicHooks, PublicMonitor, PublicPath, PublicSettings, PurgeHook,
};
pub use refine::{RefineCond, RefineLimits, RefineOp, Refinement};
pub use registry::{MapRegistry, RegistryHandle};
pub use router::{
    method_for, router, router_with_blobs, well_formed_space, ApiState, AGENT_PARAM, OP_BODY_LIMIT,
};
pub use spec::{
    AccessChecker, AccessDecision, BoundParams, Bounds, CallerKind, CostClass, DoorRefusal,
    EmailGate, ExecError, ExecOutcome, ExecResult, FieldSpec, FieldTy, Hearing, InvalidationBus,
    InvalidationEvent, NoQuota, NotSyncable, OpCost, OpExecutor, OpKind, OpRegistry, OpSpec,
    ParamSpec, ParamTy, PublicMode, QuotaDecision, QuotaGate, RefineTier, ReplayLease, SpecError,
    Subscribing, Subscription, Syncability, TokenGate, ANONYMOUS,
};
pub use sync::SyncPacing;
pub use sync::{
    DEFAULT_PAGE_ROWS as DEFAULT_SYNC_PAGE_ROWS, DEFAULT_PAGE_WAIT_MS as DEFAULT_SYNC_PAGE_WAIT_MS,
    PAGE_ROWS_BOUNDS as SYNC_PAGE_ROWS_BOUNDS, PAGE_WAIT_MS_BOUNDS as SYNC_PAGE_WAIT_MS_BOUNDS,
};
