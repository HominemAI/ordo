//! The server's edge: the one place an HTTP status and the wire error
//! envelope are chosen (`docs/audit-service-interfaces.md`, fix order
//! step 6).
//!
//! `{ "error": { "code", "message", "op"?, "param"?, ... } }`.
//!
//! Two closed inventories meet here. [`ErrorCode`] is the op door's,
//! answered through [`ApiError`]; [`EdgeCode`] is the words of the other
//! surfaces (sign-in, accounts, delegation, links), answered through
//! [`EdgeError`]. Each code carries its own status, so a handler names
//! what happened and never a number. A typed failure from a tier below
//! reaches the door by its [`FaultClass`] ([`ApiError::by_fault`],
//! [`ExecError::of_fault`]), never by its words. `ordo-host`'s
//! `tests/edge_scan.rs` fails on a status or an error envelope built
//! anywhere else.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use ordo_fault::FaultClass;
use serde_json::{Map, Value};

use crate::spec::{ExecError, OpSpec};

/// What a saturated connection pool says, word for word: the text of
/// `ordo_exec::PoolError::Saturated`, which this crate cannot name.
/// `ordo-host`'s tests hold the two equal. The door recognises a busy pool
/// by its type, never by these words.
pub const SERVER_BUSY_MESSAGE: &str = "server_busy: every database connection this deployment \
     holds is in use. Nothing ran, and the same request can be sent again shortly";

/// How long a `server_busy` response tells the client to wait, in
/// seconds. One: the pool's own acquire wait is 2s by default, so a
/// connection is usually free by the time the retry arrives.
const SERVER_BUSY_RETRY_AFTER: &str = "1";

/// [`SERVER_BUSY_MESSAGE`] without its class, which is what the envelope's
/// `message` carries: the class is already in `code`.
pub(crate) fn server_busy_message() -> &'static str {
    SERVER_BUSY_MESSAGE
        .strip_prefix("server_busy: ")
        .expect("the message is class-prefixed")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// No such registered op. **Never answered on the request path** — the
    /// router answers [`ErrorCode::AccessDenied`] for an unknown name so that
    /// no token can enumerate the deployment's op surface (C7 / X-038); see
    /// `router::door_denial`. Kept in the taxonomy for the surfaces that are
    /// legitimately allowed to say it (build tooling, the deploy diff).
    UnknownOp,
    UnknownParam,
    MissingParam,
    InvalidParam,
    BoundViolation,
    InvalidRefinement,
    /// The refinement raises the op's **effective** cost class past what it
    /// was registered for (`api-engine-design.md` §7.1, `std-library.md`
    /// §2.1 rule 4). 400, naming the field.
    ///
    /// A registered class describes the op *as written*. A caller's
    /// `$refine` changes the work it does — a condition on a sealed field
    /// that no blind index answers is a host-side decrypt of every candidate
    /// row — so the served class is recomputed per request and refused when
    /// it lands on `unbounded-host` without the op's `unbounded`
    /// acknowledgment. The marker exists so a human ships O(space) work on
    /// purpose; a refinement envelope cannot acknowledge on anyone's behalf.
    ///
    /// Distinct from [`ErrorCode::InvalidRefinement`], which is the
    /// *declaration-time* gate: whether the field is refinable at all,
    /// whether a sealed condition is equality-only, whether an aggregate-only
    /// op may take one. Those refuse a refinement nobody may write; this one
    /// refuses a refinement nobody may afford. §7.1 requires both — "the
    /// declaration-time half alone still lets an opted-in field blow the
    /// cost class".
    RefinementUnbounded,
    InvalidCursor,
    NotLive,
    MethodMismatch,
    /// The request did not bind exactly one space (SH-19 / X-040): `$space`
    /// absent from the envelope and `X-Ordo-Space` absent, ill-formed, or the
    /// two disagreeing. There is no default space, ever.
    MissingSpace,
    /// **Door-level** denial: this token may not call this op in this space.
    /// Distinct from [`ErrorCode::AccessGuard`] — see that variant.
    AccessDenied,
    /// The request envelope carried `$caller` as a client parameter
    /// (`guards-conformance.md` R-G13 / GC067–GC068). Refused **at the
    /// door**, before binding or planning, so no guard is ever evaluated
    /// against a principal the client named.
    ///
    /// 403, not 400: this is not a malformed envelope, it is an attempt to
    /// assert a principal, and the answer belongs with the other
    /// authorization refusals. It gets its own code rather than hiding
    /// inside `unknown_param` because a caller who mistakes `$caller` for
    /// an ordinary parameter must be told the difference, and because it
    /// discloses nothing: the client already knows what it sent.
    CallerSupplied,
    /// The bearer token **claims a delegation that is not live** — withdrawn
    /// by the delegator, unknown to the registry, or unconfirmable because
    /// the registry could not be reached (DL §8; `bootstrap.md` item 10).
    /// 403.
    ///
    /// # Why this is not `access_denied`, when everything else at the door is
    ///
    /// The door's uniform answer exists so a caller cannot use refusals to
    /// enumerate the deployment: an unregistered op and a forbidden op are
    /// byte-identical, because telling them apart tells a prober what exists.
    /// That argument does not reach this class, and the reason is
    /// **positional**:
    ///
    /// - It is decided from the **token alone**, before the op name and the
    ///   request's space are consulted, so the same token gets the same
    ///   answer for every name it could ever send. It therefore cannot be
    ///   used to distinguish one op from another, one space from another, or
    ///   a real name from a guess — there is nothing to correlate.
    /// - What it discloses is the state of the caller's **own** delegation,
    ///   which the caller is the holder of. It learns nothing about anybody
    ///   else's space, roles, grants, or ops.
    ///
    /// And the repair is genuinely different, which is why folding it in
    /// would be a disservice rather than a discipline. `access_denied` means
    /// *ask whoever administers you for the op*. This means *the person you
    /// were acting for has to consent again* — a different act, by a
    /// different person, that no amount of retrying or scope-widening
    /// reaches. An agent that cannot tell the two apart either retries
    /// forever or escalates to a human with the wrong question.
    ///
    /// A token that claims **no** delegation never sees this class: §8 says
    /// an agent without one *"acts only as itself"*, which is an ordinary
    /// call, not a refusal.
    DelegationRequired,
    /// **Row-level** guard rejection raised by the executor after the door
    /// opened: the caller may call the op, but a write landed on a row outside
    /// their write view (`definition-language.md` R-G10, guards conformance
    /// GC012/GC013). Also 403, deliberately a *different* code: conflating it
    /// with `access_denied` tells an attacker which layer they hit, and the
    /// two answer different questions ("may you call this?" vs "may you touch
    /// this row?").
    AccessGuard,
    /// The space this request named has used its plan's quota, plus its
    /// grace, for `requests` or `rows_returned` (17n M2). 429.
    ///
    /// **Raised at the door, immediately after [`ErrorCode::AccessDenied`]'s
    /// check and never before it** (`crate::router::gate`): a quota refusal
    /// discloses that the space exists and is busy, and handing that to a
    /// caller with no membership in the space would be the same
    /// enumeration `access_denied`'s uniform answer exists to prevent. A
    /// caller who is not a member of an over-quota space is answered
    /// `access_denied` and learns nothing about the quota
    /// (`corpus-rulings.md`, `write-quota`, 2026-09-19).
    ///
    /// Distinct from the unminted `delegation_budget`: a space over its plan
    /// and a delegation over its grant are different limits with different
    /// remedies — collapsing them would send an operator to the wrong page.
    ///
    /// **Soft by design.** A quota has a limit and a grace; over the limit
    /// alone warns and continues, and this class is raised only past the
    /// grace too. The message names the quantity the space is over and
    /// nothing about any other space.
    ///
    /// v1 gates `requests` and `rows_returned` only — both are counted at
    /// the door as they happen. `writes` is counted from the command log at
    /// rollup and a quota on it would bite up to a rollup period late, so
    /// it is deferred to 18i rather than minted here as a quota in name
    /// only.
    QuotaExceeded,
    /// A public write without a human check the provider confirmed
    /// (`design-public-path.md` §5): no `X-Ordo-Human` header, a token the
    /// provider refused, a provider that could not be asked, or no provider
    /// configured. The door's own refusal, before the executor. The message
    /// names `ORDO_PUBLIC_HUMAN_CHECK` and the secret file setting where a
    /// setting would fix it.
    HumanCheckFailed,
    /// Executor pass-through: the addressed row does not exist — or exists
    /// outside the caller's *read* view, which answers the same way, because
    /// you learn a row exists only if you may read it (SH-5 / X-032).
    NotFound,
    VersionSkew,
    /// The write asserted `rev: null` — create-once — and the unique key
    /// already matches a row (spec §6 rule 9). Nothing applied. Distinct from
    /// `Duplicate`, which is a constraint violation the caller did not
    /// anticipate; this one is a belief the caller stated and the data
    /// refused.
    ExpectedAbsent,
    /// A converge-or-create whose addressing unique key's bound value is
    /// **null** (spec §3 / §6 rule 8). 400, not 409: nothing about the
    /// stored data decided this — the request addressed no row, and the
    /// same payload will be refused for the same reason forever.
    NullKeyAddress,
    ConflictingIdentity,
    Duplicate,
    CasMiss,
    /// Same `Idempotency-Key`, different request payload (SH-17 / X-039). A
    /// loud 409 — never a replay of the first payload's response.
    ///
    /// For a **creating** op the binding is permanent (Z-018): the key stays
    /// bound to the payload that created the row, so an edited replay is this
    /// error even after the stored response expired. A creating form carries
    /// no `rev`, so nothing downstream could have caught the silent converge.
    IdempotencyMismatch,
    /// Same `Idempotency-Key`, same payload, and the first attempt is **still
    /// running** on some node (Z-012). A 409 the client retries: the second
    /// caller waited out its budget without the winner committing, and
    /// executing anyway would be the double-write the key exists to prevent.
    IdempotencyInFlight,
    /// Same `Idempotency-Key`, same payload, the effect **already
    /// committed** — and its response body went with the process that ran
    /// it. 409, and **terminal**: retrying will never produce a different
    /// answer, because nothing is still running and the body is gone.
    ///
    /// The distinction from [`ErrorCode::IdempotencyInFlight`] is the whole
    /// reason this class exists. `in_flight` says *somebody is still
    /// working; ask again*. A caller that switches on the class name and
    /// polls on it is right to poll. Here there is nobody to wait for: the
    /// write landed, the `$replay` journal recorded it in the write's own
    /// transaction, and the crash that lost the body happened after both.
    /// A caller polling this class polls forever, which is why it wore the
    /// wrong name for as long as it did.
    ///
    /// **Raised by the router, never by the executor**
    /// ([`ErrorCode::executor_raisable`] is `false`). The executor's
    /// channel for this fact is `ExecOutcome::AlreadyApplied` — an `Ok`
    /// outcome, not a refusal — and the router turns that into this class
    /// after asking the response store for a body and being told there is
    /// none. An executor that raised the class string directly would be
    /// asserting the store's emptiness from a layer that cannot see the
    /// store, so `from_exec` downgrades it to `internal` like every other
    /// door-only class.
    ///
    /// **Not cacheable for replay** ([`ErrorCode::is_exec_conflict`] is
    /// `false`), deliberately, and the tempting argument for the opposite
    /// is worth answering. It looks cacheable: it is terminal, so storing
    /// it would make every later retry cheap and identical. Two reasons it
    /// is not:
    ///
    /// - **It is not an outcome; it is the absence of one.** Every member
    ///   of the cacheable family is what the op *did* against the data at
    ///   that moment. Nothing executed here — the attempt's transaction
    ///   rolled back untouched — so there is no result to store, only a
    ///   report about the store's own contents.
    /// - **Storing it would mask a body that can still arrive.** The
    ///   holder that lost its response may be stalled rather than dead: its
    ///   reservation ages out, this retry answers, and the original then
    ///   completes its `put` with the real 200. Writing a terminal 409 into
    ///   the same slot makes the order of those two decide what every
    ///   future retry sees, and the losing order is the one that answers
    ///   *"your body is gone"* to a caller whose body is sitting in the
    ///   store. The journal is the durable authority on this key and it
    ///   answers the same way every time it is asked; caching the answer
    ///   buys nothing and adds a way to be wrong.
    IdempotencyApplied,
    /// The idempotency lease this attempt held was taken over by a newer
    /// holder while it worked, so its effect was rolled back rather than
    /// committed under a key someone else now owns. 409 and retryable: the
    /// caller's request was fine, another holder simply won the key.
    LeaseFenced,
    /// The write ran at SERIALIZABLE, lost a serialization race (`40001`),
    /// and lost it again through its whole retry budget.
    ///
    /// 409 and retryable, which is the entire point of giving it a code:
    /// `implementation.md` (Write path) pins SERIALIZABLE for bulk
    /// conditional writes and multi-statement groups, makes the retry part
    /// of that pin, and then says in as many words that "exhaustion is a
    /// *retryable conflict*, not an internal error — it needs a wire code,
    /// and the closed taxonomy does not have one yet", flagging it here as
    /// `serialization_conflict` [proposed]. This is that code, landing with
    /// the retry loop that can raise it. Nothing about the request was
    /// wrong; the deployment was simply too busy in this space right now,
    /// and the caller may try again.
    ///
    /// Not cacheable for replay ([`ErrorCode::is_exec_conflict`]): no state
    /// decided it and nothing executed, so a retry under the same
    /// idempotency key must really run.
    SerializationConflict,

    /// One statement ran past the deployment's `statement_timeout` and the
    /// server cancelled it (SQLSTATE 57014), so nothing it was doing
    /// landed. Producer: `ordo_exec::PoolError::StatementTimeout`; the
    /// bound is `ORDO_PG_STATEMENT_TIMEOUT_MS`, pinned on every pooled
    /// connection at dial (20a).
    ///
    /// **503, with `space_unavailable` and `server_busy`.** The
    /// request is well formed, so the 4xx family is wrong; the server did
    /// not break, so `internal` is wrong; and no stored row decided it, so
    /// the 409 family's "change what you collided with" does not describe
    /// the repair either. What the caller learns is that this deployment
    /// bounds a statement and theirs did not fit, which is answered by
    /// asking for less or by trying again when the database is not being
    /// held by someone else. A client that backs off and retries is doing
    /// the right thing, and 503 is the status that tells it so.
    ///
    /// Not cacheable for replay ([`ErrorCode::is_exec_conflict`]): nothing
    /// executed, and the identical call may well land next time.
    StatementTimeout,
    /// `/ 0`, `mod(a, 0)`, or an `int`/`money` overflow — DL §2 makes each
    /// "a typed runtime error", and typed is the operative word: the
    /// alternative is a 500 saying the server broke when the arithmetic
    /// merely has no answer.
    ///
    /// **Engine-pinned pending a spec ruling.** The behaviour is settled
    /// and the inventory names no class for it, so the name and the 400
    /// are this engine's choice — taken by analogy with `bound_violation`
    /// (a value outside a permitted numeric domain) and reported rather
    /// than smuggled in.
    ArithmeticError,

    /// A declared row invariant (`check used <= seats`) refused the write —
    /// SQLSTATE 23514, typed at the pool the way the arithmetic family is.
    /// The data violated the rule; the server did not break, so this must
    /// never wear `internal`. The message carries the constraint name,
    /// which carries the condition (`teams$chk0$used<=seats`) — the only
    /// channel Postgres gives wide enough to say WHICH check failed.
    ///
    /// 409, and in [`ErrorCode::is_exec_conflict`], for `duplicate`'s
    /// reason: the refusal IS what the op did against the data at that
    /// moment, so replaying the same idempotency key must answer the same
    /// refusal rather than re-execute.
    CheckViolation,

    // ── The write path's typed refusals (`writing-data.md` §7) ──────────
    //
    // Every class below was raised by the write protocol long before it had
    // a wire class, and therefore arrived as `internal` (500) — a rule that
    // IS built, indistinguishable from a server bug. The split that decides
    // each status is "what refused it": stored data (409 — the same request
    // may succeed once the data changes) or the statement itself (400 — no
    // state will ever make it succeed).
    /// Deleting a row another row still references, named with the referrer
    /// and its field (`writing-data.md` §6b). 409: remove or repoint the
    /// referrer and the identical request succeeds.
    HasReferrers,
    /// A tree move that would put a node inside its own subtree. 409: the
    /// current shape of the tree is what refused it.
    TreeCycle,
    /// Deleting a tree node that still has children. 409, for the same
    /// reason as [`ErrorCode::HasReferrers`].
    TreeHasChildren,
    /// A create or move past the tree depth cap. 409: the *parent's* current
    /// depth decided it, so re-parenting shallower makes the same write land.
    TreeDepthExceeded,
    /// A ranged move: tree nodes move one at a time. 400, not 409 — unlike
    /// its three siblings above, no state decided this. A ranged reparent is
    /// refused for every dataset there is.
    TreeBulkReparent,
    /// The statement writes `id` or `rev` — names the platform derives and
    /// an op never assigns.
    ReservedDerivedName,
    /// The braces of an unconditioned `set` cover more than one unique key
    /// (DL §6 rule 8). Each of them addresses a row and they can name
    /// different rows, so there is no single row the statement means.
    AmbiguousIdentity,
    /// A parameter or a required field reached the write with no value and
    /// no default.
    UnboundRequiredParam,
    /// The statement assigns a field the entity does not have.
    ProjectionUnknownField,
    /// A right-hand side outside the closed set (literal, param, `$caller`),
    /// or one whose shape the statement cannot mean — `rev` on a statement
    /// that addresses no single row, a subtree delete with no anchor.
    InvalidBindingExpr,
    /// A binding expression's value does not have its declared type.
    ParamTypeMismatch,
    /// A registered op names a source — an entity or a callee — that this
    /// deployment does not have.
    ///
    /// **500, and deliberately so.** The other statement-shaped classes are
    /// 400 because the caller's request can never succeed as written; this
    /// one is not about the request at all. It means a registered op
    /// survived registration referring to something absent, which is a
    /// broken deployment. It still gets its own code rather than plain
    /// `internal`, so the operator reading the log is told which kind of
    /// broken. Nothing here enumerates: the missing name comes from the op's
    /// own body, never from the caller.
    UnknownSource,
    /// A sealed value did not open — wrong key epoch, wrong column, or a
    /// tampered payload. 500: the caller did nothing wrong and can do
    /// nothing about it, but an operator must be able to tell a key-custody
    /// fault from an unknown panic.
    SealedOpenFailed,
    /// A sealed value failed to authenticate or decrypt while its key is
    /// present: unreadable, possibly tampered (owner, 2026-10-23).
    ///
    /// 500 and never retryable: sending the same call again gets the same
    /// answer. It is not `sealed_open_failed` (a custody fault) and it is
    /// never called destroyed, which means only a key someone erased on
    /// purpose. The message names the entity and field, never the value or
    /// any key material. The deployment owner is alerted on the tenant's
    /// security-event stream.
    SealedUnreadable,
    /// The write's command group could not be appended to the space's
    /// durable command chain, so the write itself was rolled back.
    ///
    /// 500, alongside [`ErrorCode::SealedOpenFailed`] and for the same
    /// reason: the caller did nothing wrong and can do nothing about it,
    /// but an operator must be able to tell an audit-spine fault from an
    /// unknown panic. The refusal is deliberate — an effect that commits
    /// without a link is a write nobody can attribute or undo, which is
    /// precisely what the log exists to prevent, so failing the request is
    /// the safe answer rather than the convenient one.
    LogAppendFailed,
    /// The op's plan needs a venue this executor does not serve, such as a
    /// sealed read with no take that bounds it.
    ///
    /// **422** (ruled 2026-10-05, `audit-2026-10-05c` ask 2). Sealed reads
    /// are served, so what is left here is an op shape the app developer
    /// fixes: add a take, add a blind index, require a signed-in caller.
    /// The request is well formed and nothing failed, so neither 400 nor
    /// 500 describes it. It was 501 while sealed reads were unbuilt.
    SealedUnsupported,
    /// The live database is not the shape the deployed artifact asserts:
    /// a migration half-applied, a restore onto an older schema, a column
    /// changed behind the deployment's back.
    ///
    /// **500, and its own code rather than `internal`.** The caller did
    /// nothing wrong and can do nothing about it — which is the 5xx
    /// property — but "your database and your artifact disagree" is the one
    /// operator action that fixes it, and an operator who reads `internal`
    /// goes looking for a panic. `ordo_host::deploy::LoadRefusal::SchemaDrift`
    /// already refused a *load* on this fact with no wire name; the space
    /// export is the first producer that meets a caller, and both are the
    /// same fact.
    SchemaDrift,
    /// An irreversible act was refused because the space still owes
    /// something: an archive that has not received the history it is the
    /// copy of record for, a client-first device that is the authoritative
    /// store, a listener with at-least-once deliveries outstanding.
    ///
    /// **409**, alongside `duplicate` and `cas_miss`: the request is
    /// well-formed and the caller may repeat it verbatim once the state it
    /// names has moved. A 4xx that never becomes possible would be the
    /// wrong shape; this one becomes possible the moment the consumer
    /// catches up or is de-registered.
    SpacePinned,
    /// A destructive op arrived without its confirmation, or with the
    /// wrong one. 400: the request is malformed as written, and no state
    /// will make this exact call succeed.
    ///
    /// Distinct from `missing_param` and `invalid_param` deliberately. Those
    /// are the door's type check; this is a refusal *after* the parameter
    /// bound correctly, made by the op because what the value says is not
    /// what the op requires it to say. Collapsing them would let a caller
    /// read "you sent the wrong type" where the truth is "you have not
    /// confirmed you meant this".
    NotConfirmed,
    /// An outbound destination was refused by one of
    /// `security-logging-design.md` §7b's egress pins: it is not `https`, it
    /// is not a usable URL, it is not on the space's consented allowlist, it
    /// does not resolve, or it resolves to an address inside the perimeter.
    ///
    /// **400**, because the value is malformed *as a destination* and no
    /// state a caller can reach makes `http://169.254.169.254/` acceptable.
    ///
    /// **One class for all six pins, on purpose.** A class per pin would be
    /// a reconnaissance primitive: `egress_denied_pod_cidr` as a distinct
    /// wire word lets anyone who can name a hostname map the deployment's
    /// internal address plan one grant at a time. The *category* is in the
    /// message, because §7b requires the deny list to be disclosed on the
    /// consent page and a refusal that will not say which promise it kept is
    /// one nobody can act on; the resolved address never is.
    EgressRefused,
    /// A chain proof was asked for over a range this deployment cannot
    /// prove — below the pruning floor, past the head, or a segment that
    /// does not recompute locally.
    ///
    /// **409**: the request is well-formed, and a narrower range succeeds.
    ///
    /// Deliberately **not** `not_found`. A range whose entries were lawfully
    /// pruned is a range about which the deployment is telling the truth, and
    /// collapsing that into "no such thing" would let a deployment answer a
    /// tampering question with a shrug — which is the one answer
    /// `security-logging-design.md` says verification must never give, since
    /// *"I could not check" and "it checks out" must not be the same value*.
    ProofUnavailable,
    /// `std.deployment.apply` refused the program before it sent anything
    /// (18l). The classes the apply path names — `ops_do_not_compile`, and
    /// every triage refusal `schema_apply::Refused` carries
    /// (`plan_refused`, `not_class_a`, `artifact_first`,
    /// `blind_index_backfill_required`, `constraint_over_existing_rows`) —
    /// travel under this one code with the class in the message.
    ///
    /// **409**: the request is well-formed, nothing was sent, and the
    /// identical call lands once the thing the refusal names has moved —
    /// the op recompiles, the offending rows are fixed, the hand statement
    /// the refusal carries is run. That is this family's property exactly.
    ///
    /// **One code for the whole set, on purpose**, and for the opposite of
    /// `egress_refused`'s reason: not secrecy, but that the classes are a
    /// *taxonomy the apply path owns and extends* — increments 3 through 5
    /// add to it — and a wire code per class would make every new refusal a
    /// breaking change to a closed inventory two tests enforce. The class
    /// is the first word of the message, in the spelling
    /// `schema_apply::Refused::class` gives it, so `ordo apply` over the
    /// wire prints what `ordo apply` over the database prints.
    ApplyRefused,
    /// `std.space.create` will not create a tenant from these values, and
    /// created nothing (17aa).
    ///
    /// **400**, not 409: the values themselves are wrong and the identical
    /// call never lands. Nothing in the deployment has to move first,
    /// which is the whole of what separates this from the apply family
    /// above.
    ///
    /// **Its own class rather than `invalid_param`**, which is the door's
    /// word and one an executor may not say ([`ErrorCode::executor_raisable`]).
    /// The distinction is not bureaucratic: the door validates a parameter
    /// against the *spec* — present, a string, under the length — and
    /// passes. What fails here is a rule the door cannot see, because it
    /// belongs to provisioning: an `owner` becomes the tenant's founding
    /// **member id**, so it must be an identifier the door could later
    /// address, and a blank `name` is the one attribute an unreadable id
    /// was supposed to be picked out by. Both are only discoverable below
    /// the door, which is `invalid_cursor`'s argument for being raisable
    /// too.
    ///
    /// **One code for the set**, on `apply_refused`'s reasoning: this is a
    /// taxonomy `crate::tenant_ops` owns and will extend, and a wire code
    /// per rule would make each new one a breaking change to a closed
    /// inventory. The message names which rule.
    TenantRefused,
    /// `std.space.attachBucket` will not attach this bucket, and attached
    /// nothing (18p U6).
    ///
    /// **400**, on `tenant_refused`'s reasoning one tier over: what is
    /// refused is the *target the caller named*, so no state in the
    /// deployment makes the identical call land. The endpoint stays off the
    /// allowlist, the deployment's own bucket stays the deployment's, and a
    /// bucket another tenant is in stays theirs until somebody detaches it.
    ///
    /// **Its own class rather than `tenant_refused`.** They look like one
    /// family and are two: `tenant_refused` is
    /// [`crate::ErrorCode::executor_raisable`]'s example of a taxonomy
    /// `ordo_host::tenant_ops` owns, and a wire class is what an
    /// operator's alerting keys off. A deployment that saw
    /// `tenant_refused` from an attach would be told a tenant was not
    /// created, which is not what happened.
    ///
    /// **One code for the whole set** — off the allowlist, the
    /// deployment's own store, a bucket another tenant is in, a space that
    /// already holds objects, a credential the bucket refused — for
    /// `tenant_refused`'s own stated reason: a class per rule would make
    /// each new rule a breaking change to a closed inventory. The message
    /// names which rule, **except** the allowlist, whose message carries
    /// one reason and no detail because a distinguishable refusal maps the
    /// allowlist one attempt at a time.
    BucketRefused,
    /// `std.space.import` will not load this bundle, and loaded nothing
    /// (11-10a).
    ///
    /// **409**, with the apply family rather than with `tenant_refused`'s
    /// 400, and the difference is which side has to move. Every class in
    /// this set names something outside the request that the identical call
    /// then lands over: deploy the artifact the bundle was exported from,
    /// import into a space that is empty, wire a blob tier, stage a bundle
    /// whose members and manifest agree. The bundle is **named** rather
    /// than supplied inline, so even a malformed one is state the caller
    /// can fix and retry against without changing a single parameter.
    ///
    /// **One code for the whole set**, on `apply_refused`'s reasoning, and
    /// the set is large: `format_unknown`, `bundle_incomplete`,
    /// `artifact_mismatch`, `field_mismatch`, `space_not_empty`,
    /// `cyclic_entities`, `tree_cycle`, `unresolved_ref`, `row_refused`,
    /// `unencodable_value`, `blob_door_missing`, `blob_unplaced`,
    /// `blob_tier_unsupported`, `blob_refused`. It is a taxonomy
    /// `ordo_exec::import` owns and extends, and the deferred work in
    /// `plan-import.md` §7 adds to it, so a wire class per refusal would
    /// make every increment a breaking change to an inventory two tests
    /// enforce. **The class is the first word of the message**, in the
    /// spelling `ImportError` gives it, so an import refused over the door
    /// reads identically to one refused in-process.
    ///
    /// **Two of `import`'s refusals are deliberately not this code.** A
    /// ciphertext bundle is `sealed_unsupported`, which already names that
    /// fact from the other side, and a bundle the destination's blob door
    /// itself refused keeps the door's own class, the way `row_refused`
    /// keeps the write protocol's.
    ImportRefused,
    /// **The database or object store this space lives in could not be
    /// reached** (18d T5 / 18p U7). The request is well formed, nothing
    /// was written, and the identical call lands when the dependency
    /// comes back.
    ///
    /// **503 rather than 500**, which is what it used to answer through
    /// `ExecError::internal`. The distinction is the one an operator and
    /// a client both act on: a 500 says the deployment is broken and
    /// somebody should look at it, and this says a dependency is down and
    /// the right response is to retry later. `statement_timeout` and
    /// `server_busy` are the other 503s in this inventory, and each says
    /// the same kind of thing about a different resource.
    ///
    /// **The message is opaque about the cause** (§9.1 R-C4.4). A dial
    /// error names a host, a port and often a user, which is deployment
    /// topology a space owner has no business reading; and for a tenant
    /// on their own cluster it is their own infrastructure being
    /// described back to them by a third party. The reason an operator
    /// needs is in `/health/ready`'s per-tenant list and in the log,
    /// which are surfaces the operator reaches and the caller does not.
    ///
    /// **Not `Rewound`'s class, deliberately.** A rewound chain is the
    /// deployment disagreeing with itself and needs a human to record a
    /// restore; no amount of waiting fixes it, so it keeps `internal`.
    /// Collapsing the two would tell a client to retry something that
    /// will never succeed.
    SpaceUnavailable,
    /// **Every database connection this deployment holds is in use**, and
    /// the request waited out the pool's bounded acquire (reported fix
    /// 25z). Producer: `ordo_exec::PoolError::Saturated`; the bound is
    /// `ORDO_PG_POOL_SIZE` and the wait is `ORDO_PG_ACQUIRE_TIMEOUT_MS`
    /// (default 2s). Nothing ran, so nothing landed.
    ///
    /// **503, retryable, and its own class.** The ruling on the pool's
    /// bound already called this "a fast typed failure the door can render
    /// retryable", and it used to answer `internal`. It is neither of the
    /// other 503s. `statement_timeout` says the caller's statement was too
    /// long, which asking for less repairs; here the statement never
    /// started. `space_unavailable` says a dependency is unreachable, which
    /// a deployment owner looks at; here the database is up and this
    /// deployment's own pool is full, which clears as the requests ahead
    /// finish. A client that waits briefly and retries is doing the right
    /// thing, and the response carries `Retry-After` to say how briefly.
    ///
    /// **The message is fixed** ([`SERVER_BUSY_MESSAGE`]) and names no
    /// pool size, no space and no host. How full the pool is, and for
    /// which space, is in `/metrics` for the deployment owner.
    ///
    /// Not cacheable for replay ([`ErrorCode::is_exec_conflict`]): nothing
    /// executed, and the identical call lands once a connection is free.
    ServerBusy,
    /// **The tenant is paused** (checklist 27ar). Somebody held it still on
    /// purpose: the deployment owner, or a tenant role the app developer
    /// granted `std.space.pause`. Its data is kept and nothing ran.
    ///
    /// **423 Locked, and its own class.** It is not `space_unavailable`:
    /// that one says a dependency is down and the identical call lands
    /// when it comes back, so a client retries. Waiting does not lift a
    /// pause; somebody has to resume the tenant, so a retry loop here would
    /// spin until a person acts. It is not `access_denied` either: the
    /// caller may call the op, and a 403 would send them looking for a
    /// permission they already hold. 423 says the resource is locked, which
    /// is exactly the state.
    ///
    /// Raised after the door's access check, so a caller with no membership
    /// in the tenant is answered `access_denied` and learns nothing about
    /// whether it is paused. Not cached for replay
    /// ([`ErrorCode::is_exec_conflict`]): after a resume the identical call
    /// lands. Ops the app marks `nopause`, and the platform ops listed in
    /// `ordo_host::pause`, keep answering.
    SpacePaused,
    /// **A backend function threw**, or answered something that is not JSON
    /// (`plan-backend-functions.md` §10, `function_error`). The app
    /// developer's code failed; the message is the function's own, scrubbed
    /// of secret values. 500: the same call fails again until the code
    /// changes.
    FunctionError,
    /// **A backend function ran past its wall time or its CPU limit**
    /// (`function_timeout`). 504.
    FunctionTimeout,
    /// **A backend function needed more memory than its limit**
    /// (`function_oom`). 500.
    FunctionOom,
    /// **Too many function calls are running** for this tenant or this
    /// server (`function_busy`, §9). 429 with `Retry-After`: the identical
    /// call lands once one finishes. Nothing ran.
    FunctionBusy,
    /// **The person's own tenant is still being set up** (owner, 2026-10-21:
    /// "nothing can be written to the tenant"). Its registry row is
    /// `provisioning`, so nothing in it can run yet. Nothing ran.
    ///
    /// **503, retryable, with `Retry-After`.** It is the same claim the
    /// other 503s make: the request is well formed and lands unchanged once
    /// the tenant is ready. Its own class, because the cause is neither a
    /// full pool nor an unreachable database: the tenant does not exist
    /// yet, and `GET /auth/provisioning` says how far it got.
    ///
    /// Answered by the door, after the access check denied, and only to the
    /// person the tenant is being made for. Anyone else gets the door's
    /// uniform `access_denied`, so the class says nothing about a tenant the
    /// caller has no claim to. The wait is `ORDO_PROVISION_NOT_READY_RETRY_SECS`.
    TenantNotReady,
    /// **The person has no verified email address, and the app developer
    /// requires one for this op** (owner ruling 2026-10-22: "app dev could
    /// make it required"). Nothing ran.
    ///
    /// **403, its own class.** `access_denied` means ask whoever
    /// administers you; this means add an address and confirm it with an
    /// emailed code (`POST /auth/email/begin`), which the person does
    /// alone. The message names the setting that requires it
    /// (`ORDO_EMAIL_REQUIRED` or `std.deployment.setEmailPolicy`).
    ///
    /// Answered by the door, after the access check allowed the call, so a
    /// caller who may not call the op learns nothing from it. It discloses
    /// only the caller's own state.
    EmailRequired,
    /// **A deployment person's touch is too old for this op**
    /// (`plan-identity-service.md` 4.3). The token is the identity
    /// service's, it verified, and the op is one its holder may call; only
    /// the passkey touch inside the session (`touched_at`) is older than the
    /// op allows. Nothing ran.
    ///
    /// **403, its own class.** `access_denied` means ask whoever administers
    /// you; this means touch your passkey at the identity service
    /// (`POST /touch`) and send the same call with the new token. Answered
    /// by the door after the access check denied, and only to a deployment
    /// token that would be allowed with a fresh touch, so it tells a caller
    /// nothing it could not already read from its own token.
    ///
    /// The same word as the sign-in surfaces' `presence_required`
    /// ([`EdgeCode::PresenceRequired`]), on purpose: it means the same thing
    /// and the client acts on it the same way.
    PresenceRequired,
    /// Another apply already holds this deployment (18l). Applies
    /// serialize on the `$migrations` advisory lock, so two of them queue
    /// rather than interleave; this one waited out its bound.
    ///
    /// **409, and its own class rather than `serialization_conflict`.**
    /// That one is a transaction the engine will retry for you. This is a
    /// deploy that must not be retried automatically, because the apply
    /// that holds the lock may be about to change the very schema this one
    /// planned against — so the retry has to re-plan, which only the
    /// caller can decide to do.
    ApplyInProgress,
    /// The artifact was stored and **not** activated, because the load gate
    /// would refuse it over this database (18l). The blocking changes are
    /// in the message, each with its diff code.
    ///
    /// **409**: nothing is wrong with the request, and it lands unchanged
    /// once the schema is moved (`ordo migrate --apply`). Its own class
    /// rather than `apply_refused`, because the outcomes differ in the one
    /// way an operator cares about — here the artifact *is* stored, so the
    /// next step is a migration and not a recompile.
    NotActivated,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::UnknownOp => "unknown_op",
            ErrorCode::UnknownParam => "unknown_param",
            ErrorCode::MissingParam => "missing_param",
            ErrorCode::InvalidParam => "invalid_param",
            ErrorCode::BoundViolation => "bound_violation",
            ErrorCode::InvalidRefinement => "invalid_refinement",
            ErrorCode::RefinementUnbounded => "refinement_unbounded",
            ErrorCode::InvalidCursor => "invalid_cursor",
            ErrorCode::NotLive => "not_live",
            ErrorCode::MethodMismatch => "method_mismatch",
            ErrorCode::MissingSpace => "missing_space",
            ErrorCode::AccessDenied => "access_denied",
            ErrorCode::CallerSupplied => "caller_supplied",
            ErrorCode::DelegationRequired => "delegation_required",
            ErrorCode::AccessGuard => "access_guard",
            ErrorCode::QuotaExceeded => "quota_exceeded",
            ErrorCode::HumanCheckFailed => "human_check_failed",
            ErrorCode::NotFound => "not_found",
            ErrorCode::VersionSkew => "version_skew",
            ErrorCode::ExpectedAbsent => "expected_absent",
            ErrorCode::NullKeyAddress => "null_key_address",
            ErrorCode::ConflictingIdentity => "conflicting_identity",
            ErrorCode::Duplicate => "duplicate",
            ErrorCode::CasMiss => "cas_miss",
            ErrorCode::IdempotencyMismatch => "idempotency_mismatch",
            ErrorCode::IdempotencyInFlight => "idempotency_in_flight",
            ErrorCode::IdempotencyApplied => "idempotency_applied",
            ErrorCode::LeaseFenced => "lease_fenced",
            ErrorCode::SerializationConflict => "serialization_conflict",
            ErrorCode::StatementTimeout => "statement_timeout",
            ErrorCode::ArithmeticError => "arithmetic_error",
            ErrorCode::CheckViolation => "check_violation",
            ErrorCode::HasReferrers => "has_referrers",
            ErrorCode::TreeCycle => "tree_cycle",
            ErrorCode::TreeHasChildren => "tree_has_children",
            ErrorCode::TreeDepthExceeded => "tree_depth_exceeded",
            ErrorCode::TreeBulkReparent => "tree_bulk_reparent",
            ErrorCode::ReservedDerivedName => "reserved_derived_name",
            ErrorCode::AmbiguousIdentity => "ambiguous_identity",
            ErrorCode::UnboundRequiredParam => "unbound_required_param",
            ErrorCode::ProjectionUnknownField => "projection_unknown_field",
            ErrorCode::InvalidBindingExpr => "invalid_binding_expr",
            ErrorCode::ParamTypeMismatch => "param_type_mismatch",
            ErrorCode::UnknownSource => "unknown_source",
            ErrorCode::SealedOpenFailed => "sealed_open_failed",
            ErrorCode::SealedUnreadable => "sealed_unreadable",
            ErrorCode::LogAppendFailed => "log_append_failed",
            ErrorCode::SealedUnsupported => "sealed_unsupported",
            ErrorCode::SchemaDrift => "schema_drift",
            ErrorCode::SpacePinned => "space_pinned",
            ErrorCode::NotConfirmed => "not_confirmed",
            ErrorCode::EgressRefused => "egress_refused",
            ErrorCode::ProofUnavailable => "proof_unavailable",
            ErrorCode::ApplyRefused => "apply_refused",
            ErrorCode::TenantRefused => "tenant_refused",
            ErrorCode::BucketRefused => "bucket_refused",
            ErrorCode::ImportRefused => "import_refused",
            ErrorCode::SpaceUnavailable => "space_unavailable",
            ErrorCode::ServerBusy => "server_busy",
            ErrorCode::SpacePaused => "space_paused",
            ErrorCode::FunctionError => "function_error",
            ErrorCode::FunctionTimeout => "function_timeout",
            ErrorCode::FunctionOom => "function_oom",
            ErrorCode::FunctionBusy => "function_busy",
            ErrorCode::TenantNotReady => "tenant_not_ready",
            ErrorCode::EmailRequired => "email_required",
            ErrorCode::PresenceRequired => "presence_required",
            ErrorCode::ApplyInProgress => "apply_in_progress",
            ErrorCode::NotActivated => "not_activated",
            ErrorCode::Internal => "internal",
        }
    }

    /// The whole inventory, in wire order. This is what [`ErrorCode::from_class`]
    /// scans, so a code that is not listed here has no way back from a class
    /// string — see the round-trip test at the bottom of this file, which is
    /// what keeps the two in step.
    pub const ALL: &'static [ErrorCode] = &[
        ErrorCode::UnknownOp,
        ErrorCode::UnknownParam,
        ErrorCode::MissingParam,
        ErrorCode::InvalidParam,
        ErrorCode::BoundViolation,
        ErrorCode::InvalidRefinement,
        ErrorCode::RefinementUnbounded,
        ErrorCode::InvalidCursor,
        ErrorCode::NotLive,
        ErrorCode::MethodMismatch,
        ErrorCode::MissingSpace,
        ErrorCode::AccessDenied,
        ErrorCode::CallerSupplied,
        ErrorCode::DelegationRequired,
        ErrorCode::AccessGuard,
        ErrorCode::QuotaExceeded,
        ErrorCode::HumanCheckFailed,
        ErrorCode::NotFound,
        ErrorCode::VersionSkew,
        ErrorCode::ExpectedAbsent,
        ErrorCode::NullKeyAddress,
        ErrorCode::ConflictingIdentity,
        ErrorCode::Duplicate,
        ErrorCode::CasMiss,
        ErrorCode::IdempotencyMismatch,
        ErrorCode::IdempotencyInFlight,
        ErrorCode::IdempotencyApplied,
        ErrorCode::LeaseFenced,
        ErrorCode::SerializationConflict,
        ErrorCode::StatementTimeout,
        ErrorCode::ArithmeticError,
        ErrorCode::CheckViolation,
        ErrorCode::HasReferrers,
        ErrorCode::TreeCycle,
        ErrorCode::TreeHasChildren,
        ErrorCode::TreeDepthExceeded,
        ErrorCode::TreeBulkReparent,
        ErrorCode::ReservedDerivedName,
        ErrorCode::AmbiguousIdentity,
        ErrorCode::UnboundRequiredParam,
        ErrorCode::ProjectionUnknownField,
        ErrorCode::InvalidBindingExpr,
        ErrorCode::ParamTypeMismatch,
        ErrorCode::UnknownSource,
        ErrorCode::SealedOpenFailed,
        ErrorCode::SealedUnreadable,
        ErrorCode::LogAppendFailed,
        ErrorCode::SealedUnsupported,
        ErrorCode::SchemaDrift,
        ErrorCode::SpacePinned,
        ErrorCode::NotConfirmed,
        ErrorCode::EgressRefused,
        ErrorCode::ProofUnavailable,
        ErrorCode::ApplyRefused,
        ErrorCode::TenantRefused,
        ErrorCode::BucketRefused,
        ErrorCode::ImportRefused,
        ErrorCode::SpaceUnavailable,
        ErrorCode::ServerBusy,
        ErrorCode::SpacePaused,
        ErrorCode::FunctionError,
        ErrorCode::FunctionTimeout,
        ErrorCode::FunctionOom,
        ErrorCode::FunctionBusy,
        ErrorCode::TenantNotReady,
        ErrorCode::EmailRequired,
        ErrorCode::PresenceRequired,
        ErrorCode::ApplyInProgress,
        ErrorCode::NotActivated,
        ErrorCode::Internal,
    ];

    /// The inventory read backwards: a class name → its code.
    ///
    /// The tiers below the transport raise their refusals as
    /// `"<class>: <message>"` strings (the write protocol, the pooled
    /// executor), and this is the only supported way back. See
    /// `ordo-host`'s `classify`, and the hazard note attached to it.
    pub fn from_class(class: &str) -> Option<ErrorCode> {
        ErrorCode::ALL.iter().copied().find(|c| c.as_str() == class)
    }

    /// May the **executor** answer with this class?
    ///
    /// The door's words are the door's alone: an executor that answered
    /// `access_denied` would be claiming an authority it does not have and
    /// telling the caller which layer they hit (§4.1 non-enumerability).
    /// The envelope's binding classes are likewise decided before the
    /// executor is reached, and `idempotency_in_flight` belongs to the
    /// replay registry, which decides it when nothing has executed at all.
    /// `idempotency_applied` is the router's for the same reason: the
    /// executor reports the fact as `ExecOutcome::AlreadyApplied`, an `Ok`
    /// outcome, and the class is only decided after the response store has
    /// been asked for a body and had none.
    ///
    /// [`ApiError::from_exec`] enforces this: a refusal carrying a class the
    /// executor may not say becomes `internal`, because an executor saying
    /// it is a bug in the executor, not an answer for the caller.
    pub fn executor_raisable(self) -> bool {
        match self {
            ErrorCode::UnknownOp
            | ErrorCode::UnknownParam
            | ErrorCode::MissingParam
            | ErrorCode::BoundViolation
            | ErrorCode::InvalidRefinement
            | ErrorCode::RefinementUnbounded
            | ErrorCode::NotLive
            | ErrorCode::MethodMismatch
            | ErrorCode::MissingSpace
            | ErrorCode::AccessDenied
            | ErrorCode::CallerSupplied
            | ErrorCode::DelegationRequired
            // The door's own second-stage refusal (`crate::router::gate`,
            // after `check_access`): the executor never sees a request the
            // quota gate refused.
            | ErrorCode::QuotaExceeded
            | ErrorCode::HumanCheckFailed
            | ErrorCode::IdempotencyInFlight
            // The router's, for the same reason: it is decided by asking the
            // response store, which the executor cannot see.
            | ErrorCode::IdempotencyApplied
            // The door's, after the access check: only the door knows which
            // person a tenant being set up belongs to.
            | ErrorCode::TenantNotReady
            // The door's, after the access check: only the door knows who
            // is calling and what the app developer requires.
            | ErrorCode::EmailRequired
            | ErrorCode::PresenceRequired => false,
            // `invalid_cursor` IS the executor's to say as well as the
            // door's: a cursor presented against a plan with no total order
            // is only discoverable once the plan exists.
            //
            // `invalid_param` too: a value the database refuses as data
            // (SQLSTATE class 22, a `matches` pattern above all) is only
            // discoverable when the statement runs (`PoolError::InvalidInput`).
            ErrorCode::InvalidParam
            | ErrorCode::InvalidCursor
            | ErrorCode::AccessGuard
            | ErrorCode::NotFound
            | ErrorCode::VersionSkew
            | ErrorCode::ExpectedAbsent
            | ErrorCode::NullKeyAddress
            | ErrorCode::ArithmeticError
            | ErrorCode::CheckViolation
            | ErrorCode::ConflictingIdentity
            | ErrorCode::Duplicate
            | ErrorCode::CasMiss
            | ErrorCode::IdempotencyMismatch
            | ErrorCode::LeaseFenced
            | ErrorCode::SerializationConflict
            | ErrorCode::StatementTimeout
            | ErrorCode::HasReferrers
            | ErrorCode::TreeCycle
            | ErrorCode::TreeHasChildren
            | ErrorCode::TreeDepthExceeded
            | ErrorCode::TreeBulkReparent
            | ErrorCode::ReservedDerivedName
            | ErrorCode::AmbiguousIdentity
            | ErrorCode::UnboundRequiredParam
            | ErrorCode::ProjectionUnknownField
            | ErrorCode::InvalidBindingExpr
            | ErrorCode::ParamTypeMismatch
            | ErrorCode::UnknownSource
            | ErrorCode::SealedOpenFailed
            | ErrorCode::SealedUnreadable
            | ErrorCode::LogAppendFailed
            | ErrorCode::SealedUnsupported
            | ErrorCode::SchemaDrift
            | ErrorCode::SpacePinned
            | ErrorCode::NotConfirmed
            // Both are a platform op's, and a platform op runs as an
            // executor: `attestTo` is the only producer of the first and
            // `chainProof` of the second.
            | ErrorCode::EgressRefused
            | ErrorCode::ProofUnavailable
            // 17aa. `std.space.create` is a platform op and its only
            // producer; see the variant for why it is not `invalid_param`.
            // 18p U6. `std.space.attachBucket` is a platform op and its
            // only producer.
            | ErrorCode::BucketRefused
            | ErrorCode::TenantRefused
            // 11-10a. `std.space.import` is a platform op and its only
            // producer; the bundle's defects are all below the door.
            | ErrorCode::ImportRefused
            // The resolver's, below the door: only a resolve knows a
            // target could not be reached.
            | ErrorCode::SpaceUnavailable
            // The pool's, below the door: only a checkout knows every
            // connection is taken.
            | ErrorCode::ServerBusy
            // 27ar. The pause gate sits below the door, in the platform
            // executor, after the access check has opened.
            | ErrorCode::SpacePaused
            // The function host's, below the door: only a run knows how it
            // ended.
            | ErrorCode::FunctionError
            | ErrorCode::FunctionTimeout
            | ErrorCode::FunctionOom
            | ErrorCode::FunctionBusy
            // 18l's three. `std.deployment.apply` is a platform op, and a
            // platform op runs as an executor; it is their only producer.
            | ErrorCode::ApplyRefused
            | ErrorCode::ApplyInProgress
            | ErrorCode::NotActivated
            | ErrorCode::Internal => true,
        }
    }

    pub fn status(self) -> StatusCode {
        match self {
            ErrorCode::UnknownOp => StatusCode::NOT_FOUND,
            ErrorCode::UnknownParam
            | ErrorCode::MissingParam
            | ErrorCode::InvalidParam
            | ErrorCode::BoundViolation
            | ErrorCode::InvalidRefinement
            | ErrorCode::RefinementUnbounded
            | ErrorCode::InvalidCursor
            | ErrorCode::MissingSpace
            | ErrorCode::NullKeyAddress
            | ErrorCode::NotLive
            // The arithmetic has no answer; the server is not broken.
            | ErrorCode::ArithmeticError
            // Statement-shaped refusals: the statement, not the stored
            // data, is what refused them, so no state will ever make the
            // identical call succeed. That is the 4xx family's property.
            | ErrorCode::TreeBulkReparent
            | ErrorCode::ReservedDerivedName
            | ErrorCode::AmbiguousIdentity
            | ErrorCode::UnboundRequiredParam
            | ErrorCode::ProjectionUnknownField
            | ErrorCode::InvalidBindingExpr
            | ErrorCode::ParamTypeMismatch
            // Bound correctly and still refused: the op requires the value
            // to SAY something, and it does not.
            | ErrorCode::NotConfirmed
            // Malformed as a destination. No state makes
            // `http://169.254.169.254/` acceptable, which is the 4xx
            // family's property.
            | ErrorCode::EgressRefused
            // The values are wrong rather than the moment: no state in the
            // deployment makes a blank name, or an owner the door cannot
            // address, into a tenant. The 4xx family's property.
            | ErrorCode::TenantRefused
            | ErrorCode::BucketRefused => StatusCode::BAD_REQUEST,
            ErrorCode::MethodMismatch => StatusCode::METHOD_NOT_ALLOWED,
            ErrorCode::AccessDenied
            | ErrorCode::CallerSupplied
            | ErrorCode::DelegationRequired
            | ErrorCode::HumanCheckFailed
            | ErrorCode::EmailRequired
            | ErrorCode::PresenceRequired
            | ErrorCode::AccessGuard => StatusCode::FORBIDDEN,
            // The one 429 in the inventory. The space may call the op — this
            // is not a `access_denied`/`access_guard` question — it has
            // simply used its plan's `requests` or `rows_returned` quota,
            // plus grace, for the period.
            ErrorCode::QuotaExceeded => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::VersionSkew
            | ErrorCode::ExpectedAbsent
            | ErrorCode::ConflictingIdentity
            | ErrorCode::Duplicate
            | ErrorCode::CasMiss
            | ErrorCode::IdempotencyMismatch
            | ErrorCode::IdempotencyInFlight
            // Terminal rather than retryable, and 409 anyway: the effect is
            // committed, so no state a caller can reach changes the answer.
            // The status says "your request collided with what is stored";
            // the class is what says "and this will never change".
            | ErrorCode::IdempotencyApplied
            | ErrorCode::LeaseFenced
            // Nothing decided this but load: the retry budget ran out.
            // Retryable, and 409 for the same reason the rest of this
            // family is — the identical request may land next time.
            | ErrorCode::SerializationConflict
            // State-decided refusals: change the data the request collided
            // with and the identical request lands.
            | ErrorCode::CheckViolation
            | ErrorCode::HasReferrers
            | ErrorCode::TreeCycle
            | ErrorCode::TreeHasChildren
            | ErrorCode::TreeDepthExceeded
            // The same family read once more: a consumer that pins an
            // irreversible act stops pinning it the moment it catches up
            // or is de-registered, and the identical request then lands.
            | ErrorCode::SpacePinned
            // A range this deployment cannot prove. The request is
            // well-formed and a narrower one lands, which is exactly this
            // family's property — and the class, not the status, is what
            // says "the entries are gone, not missing".
            // The deploy family, 18l. Each is well-formed, sent nothing,
            // and lands unchanged once the state it names has moved: the
            // op recompiles, the other apply finishes, the schema is
            // migrated. That is this family's property.
            | ErrorCode::ApplyRefused
            | ErrorCode::ApplyInProgress
            | ErrorCode::NotActivated
            // 11-10a, the same family one tier down: the bundle is named
            // rather than supplied, so every class names state the caller
            // fixes and retries the identical call over.
            | ErrorCode::ImportRefused
            | ErrorCode::ProofUnavailable => StatusCode::CONFLICT,
            // An op shape the app developer fixes: the request is well
            // formed and the op as written cannot be served (ruled
            // 2026-10-05). The one 422 in the inventory.
            ErrorCode::SealedUnsupported => StatusCode::UNPROCESSABLE_ENTITY,
            // 27ar. Held still on purpose; see the variant for why not 503
            // and not 403.
            ErrorCode::SpacePaused => StatusCode::LOCKED,
            ErrorCode::FunctionError | ErrorCode::FunctionOom => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::FunctionTimeout => StatusCode::GATEWAY_TIMEOUT,
            ErrorCode::FunctionBusy => StatusCode::TOO_MANY_REQUESTS,
            // The three 503s, and they are the same claim about different
            // resources: the work was not done, nothing broke, and the
            // identical call lands once the resource is back. See each
            // variant for why the 4xx, 409 and 500 families describe it
            // wrongly.
            ErrorCode::StatementTimeout
            | ErrorCode::SpaceUnavailable
            | ErrorCode::ServerBusy
            | ErrorCode::TenantNotReady => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::UnknownSource
            | ErrorCode::SealedOpenFailed
            | ErrorCode::SealedUnreadable
            | ErrorCode::LogAppendFailed
            | ErrorCode::SchemaDrift
            | ErrorCode::Internal => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    /// Executor conflict outcomes — cacheable for idempotent replay because
    /// they *are* the op's result (unlike validation noise or `internal`).
    ///
    /// `not_found` and `access_guard` are deliberately **not** cacheable: both
    /// are decided against state the retry re-evaluates (the row may have been
    /// created, the guard view may have changed), and a cached 403/404 would
    /// keep answering long after the fact. `idempotency_mismatch`,
    /// `idempotency_in_flight` and `idempotency_applied` are not executor
    /// outcomes at all — nothing executed. The last of those is the
    /// interesting one, because it is terminal and therefore *looks*
    /// cacheable; its variant doc gives the two reasons it is not, the
    /// second being that a stalled holder's `put` can still land the real
    /// body afterwards, and a cached 409 in that slot would answer "your
    /// body is gone" to a caller whose body is sitting in the store.
    ///
    /// The write path's state-decided refusals (`has_referrers`, the three
    /// data-decided `tree_*` classes) stay out for the `not_found` reason,
    /// not the `cas_miss` one: the caller stated no belief the data refused,
    /// the rows they collided with simply existed at that moment, and a
    /// retry re-reads them. `lease_fenced` is out because its effect was
    /// rolled back — caching it would answer "someone else holds this key"
    /// forever.
    pub fn is_exec_conflict(self) -> bool {
        // `expected_absent` joins them for the same reason `cas_miss` is
        // here: the caller stated a belief, the data refused it, and that
        // refusal IS what the op did — replaying the key must answer the
        // same thing rather than re-executing a create-once write.
        matches!(
            self,
            ErrorCode::ConflictingIdentity
                | ErrorCode::Duplicate
                | ErrorCode::CasMiss
                | ErrorCode::ExpectedAbsent
                | ErrorCode::CheckViolation
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub op: Option<String>,
    pub param: Option<String>,
    /// `Allow` header value — set on `method_mismatch` so the 405 names the
    /// correct method on the wire, not just in the message.
    pub allow: Option<&'static str>,
    /// A status that replaces the class's own on the wire. Set only for a
    /// `space_paused` the app developer gave another status (4xx or 503).
    pub status: Option<StatusCode>,
    /// `Retry-After`, in seconds, for a refusal whose wait is a setting
    /// rather than the class's own (`tenant_not_ready`).
    pub retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ApiError {
            code,
            message: message.into(),
            op: None,
            param: None,
            allow: None,
            status: None,
            retry_after: None,
        }
    }

    pub fn for_op(code: ErrorCode, op: &OpSpec, message: impl Into<String>) -> Self {
        ApiError {
            code,
            message: message.into(),
            op: Some(op.name.clone()),
            param: None,
            allow: None,
            status: None,
            retry_after: None,
        }
    }

    /// Name the op on an error built without a spec — the door's denial for
    /// an unregistered name has no `OpSpec` to borrow, and must still look
    /// exactly like the denial for a registered one.
    pub fn with_op(mut self, op: impl Into<String>) -> Self {
        self.op = Some(op.into());
        self
    }

    pub fn with_param(mut self, param: impl Into<String>) -> Self {
        self.param = Some(param.into());
        self
    }

    pub fn with_allow(mut self, method: &'static str) -> Self {
        self.allow = Some(method);
        self
    }

    /// The literal `unknown_op` answer. **Not for the request path** — the
    /// router answers unknown names with `access_denied` so an unknown op and
    /// a forbidden op are indistinguishable (C7 / X-038). Build tooling and
    /// the deploy diff, which speak to an operator who already knows the
    /// deployment, may say it plainly.
    pub fn unknown_op(name: &str) -> Self {
        ApiError {
            code: ErrorCode::UnknownOp,
            message: format!("no registered op named `{name}`"),
            op: Some(name.to_string()),
            param: None,
            allow: None,
            status: None,
            retry_after: None,
        }
    }

    /// The executor's typed refusal, passed through untranslated — except
    /// for a class the executor may not say, which becomes `internal`
    /// ([`ErrorCode::executor_raisable`]). Nothing below the door may
    /// borrow the door's answer.
    ///
    /// The code is read from the refusal's type and nothing else. A
    /// saturated pool reaches here as `server_busy` because the tier that
    /// met it said so by type ([`ExecError::of_fault`], reported fix 25z);
    /// the door does not read the message to find one.
    pub fn from_exec(op: &OpSpec, err: ExecError) -> Self {
        let code = if err.code.executor_raisable() {
            err.code
        } else {
            ErrorCode::Internal
        };
        let mut e = ApiError::for_op(code, op, err.message);
        e.status = paused_status(code, err.status);
        e
    }

    /// A typed fault from a tier below, by its [`FaultClass`]: a busy or
    /// retryable fault is `busy` (a store's own busy answer, or the plain
    /// [`ApiError::server_busy`]); every other class is `broken`. The one
    /// place a fault class becomes the door's answer, so a store, a blob
    /// tier and custody answer a full pool the same way.
    pub fn by_fault(class: FaultClass, busy: ApiError, broken: ApiError) -> ApiError {
        if class.may_retry() {
            busy
        } else {
            broken
        }
    }

    /// The `tenant_not_ready` answer: the person's own tenant is still being
    /// set up. `retry_after` is the wait the setting names, in seconds.
    pub fn tenant_not_ready(op: &str, retry_after: u64) -> Self {
        let mut e = ApiError::new(
            ErrorCode::TenantNotReady,
            "your tenant is still being set up, so nothing in it can run yet. Nothing ran. \
             Send the same request again after the Retry-After wait; GET /auth/provisioning \
             says how far setting up has got",
        )
        .with_op(op);
        e.retry_after = Some(retry_after.max(1));
        e
    }

    /// The `email_required` answer: the app developer requires a verified
    /// email address for `op`, and the caller has none. `message` names the
    /// setting.
    pub fn email_required(op: &str, message: &str) -> Self {
        ApiError::new(ErrorCode::EmailRequired, message).with_op(op)
    }

    /// The `presence_required` answer at the op door: a deployment person's
    /// touch is older than `op` allows.
    pub fn presence_required(op: &str) -> Self {
        ApiError::new(
            ErrorCode::PresenceRequired,
            "this op needs a passkey touch newer than the one in your token. Nothing ran. \
             Touch your passkey at the identity service (POST /touch, or `ordo login \
             --touch`) and send the same call with the new token",
        )
        .with_op(op)
    }

    /// The address table could not be read, so an op that requires an
    /// address is refused. A retryable 503 with `Retry-After`; the message
    /// names the setting.
    pub fn email_unreadable(op: &str, message: &str) -> Self {
        let mut e = ApiError::new(ErrorCode::SpaceUnavailable, message).with_op(op);
        e.retry_after = Some(5);
        e
    }

    /// The `server_busy` answer, for a surface that saw the pool's error
    /// itself rather than through an executor.
    pub fn server_busy() -> Self {
        ApiError::new(ErrorCode::ServerBusy, server_busy_message())
    }

    /// The wire envelope, also what the replay cache stores for conflicts.
    pub fn envelope(&self) -> Value {
        let mut inner = Map::new();
        inner.insert("code".into(), Value::String(self.code.as_str().into()));
        inner.insert(
            "message".into(),
            Value::String(caller_safe(&self.message).into_owned()),
        );
        if let Some(op) = &self.op {
            inner.insert("op".into(), Value::String(op.clone()));
        }
        if let Some(param) = &self.param {
            inner.insert("param".into(), Value::String(param.clone()));
        }
        let mut outer = Map::new();
        outer.insert("error".into(), Value::Object(inner));
        Value::Object(outer)
    }
}

/// What a caller reads in place of a message that carried the database's
/// own words.
pub const DATABASE_WORDS_HIDDEN: &str = "the database could not answer this request. Its own \
     words stay in the deployment's log; send the request again, and if it fails the same \
     way, tell the deployment owner";

/// Does `message` carry the database's own words: the pool's `db:` prefix,
/// or a server severity (`ERROR:`, `FATAL:`)?
pub fn carries_database_words(message: &str) -> bool {
    message.starts_with("db:")
        || message.contains(" db: ")
        || message.contains("ERROR: ")
        || message.contains("FATAL: ")
}

/// The one place at the edge where a message becomes a body: a message
/// that carries the database's own words is logged and replaced
/// ([`DATABASE_WORDS_HIDDEN`]). Every envelope ([`ApiError`],
/// [`EdgeError`]) goes through it, so no `db:` text reaches a caller
/// whichever layer let it through.
pub fn caller_safe(message: &str) -> std::borrow::Cow<'_, str> {
    if carries_database_words(message) {
        eprintln!(
            "ordo: an error answer carried the database's words; the caller got less: {message}"
        );
        std::borrow::Cow::Borrowed(DATABASE_WORDS_HIDDEN)
    } else {
        std::borrow::Cow::Borrowed(message)
    }
}

/// The status the app developer set for a paused tenant, when it is one the
/// setting allows: 4xx or 503. Anything else keeps the class's own 423.
pub fn paused_status(code: ErrorCode, status: Option<u16>) -> Option<StatusCode> {
    if code != ErrorCode::SpacePaused {
        return None;
    }
    status
        .filter(|s| (400..=499).contains(s) || *s == 503)
        .and_then(|s| StatusCode::from_u16(s).ok())
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status.unwrap_or_else(|| self.code.status());
        let mut response = (status, Json(self.envelope())).into_response();
        if let Some(allow) = self.allow {
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static(allow));
        }
        if matches!(self.code, ErrorCode::ServerBusy | ErrorCode::FunctionBusy) {
            response.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_static(SERVER_BUSY_RETRY_AFTER),
            );
        }
        retry_after(&mut response, self.retry_after);
        response
    }
}

fn retry_after(response: &mut Response, secs: Option<u64>) {
    if let Some(secs) = secs {
        if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, v);
        }
    }
}

impl ExecError {
    /// A typed fault from a tier below, as the executor's refusal: a busy
    /// fault is `server_busy`, and every other class is `otherwise` with
    /// `message`. What every op that meets a store's typed error answers
    /// with, so a full pool is `server_busy` because its type said so.
    pub fn of_fault(class: FaultClass, otherwise: ErrorCode, message: impl Into<String>) -> Self {
        if class.is_busy() {
            ExecError::server_busy()
        } else if class == FaultClass::Unreadable {
            // Unreadable, possibly tampered: its own non-retryable code,
            // whatever the op would otherwise have said.
            ExecError::new(ErrorCode::SealedUnreadable, message)
        } else {
            ExecError::new(otherwise, message)
        }
    }
}

/// The words of the HTTP surfaces outside the op door: sign-in, accounts,
/// delegation, links, shares. A closed inventory of its own, beside
/// [`ErrorCode`], because these are not op classes: a client of the op door
/// never sees them, and `ErrorCode::ALL` is held to the op inventory's
/// document. Each word carries its status, so no handler picks one.
///
/// `#[non_exhaustive]`: a word added later is decided by every match
/// outside this crate through its wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EdgeCode {
    // 400: the request as written.
    /// A delegation request whose body is not one.
    InvalidRequest,
    /// Tenant sign-in providers are off; names the setting.
    TenantProvidersOff,
    /// A method that carries no two-step claim was asked for one.
    PresenceNotAvailable,
    // 401: the one sign-in refusal, and its few named cousins.
    /// The one sign-in refusal. It says nothing about why.
    AuthenticationFailed,
    /// The deployment or the tenant does not keep people signed in.
    RememberOff,
    /// A link that needs its password was opened without one.
    LinkPasswordRequired,
    /// A link's password was wrong.
    LinkPasswordWrong,
    // 403: the person may not, and the message says what would let them.
    PasswordChangeRequired,
    ReauthenticationRequired,
    PresenceRequired,
    DelegationRefused,
    DelegationDeclined,
    LinkRefused,
    LinkingOff,
    EmailUnavailable,
    GuestsOff,
    CodesDisabled,
    ShareRefused,
    InvitationRefused,
    // 409: the person's own state stands in the way.
    RecoveryContestable,
    IdentifierTaken,
    LastAuthenticator,
    OriginatingAdmin,
    MembershipsRemain,
    // 410: the request is over.
    DelegationExpired,
    DelegationRedeemed,
    // 429: too many, wait.
    DelegationLimit,
    LinkLocked,
    TooManyAttempts,
    // 503: a dependency did not answer. Nothing the caller did.
    AuthUnconfigured,
    CredentialStoreUnavailable,
    SessionStoreUnavailable,
    DelegationStoreUnavailable,
    LinkUnavailable,
    TicketsUnavailable,
    ShareUnavailable,
    IdentityProviderUnavailable,
}

impl EdgeCode {
    /// The whole inventory, in wire order.
    pub const ALL: &'static [EdgeCode] = &[
        EdgeCode::InvalidRequest,
        EdgeCode::TenantProvidersOff,
        EdgeCode::PresenceNotAvailable,
        EdgeCode::AuthenticationFailed,
        EdgeCode::RememberOff,
        EdgeCode::LinkPasswordRequired,
        EdgeCode::LinkPasswordWrong,
        EdgeCode::PasswordChangeRequired,
        EdgeCode::ReauthenticationRequired,
        EdgeCode::PresenceRequired,
        EdgeCode::DelegationRefused,
        EdgeCode::DelegationDeclined,
        EdgeCode::LinkRefused,
        EdgeCode::LinkingOff,
        EdgeCode::EmailUnavailable,
        EdgeCode::GuestsOff,
        EdgeCode::CodesDisabled,
        EdgeCode::ShareRefused,
        EdgeCode::InvitationRefused,
        EdgeCode::RecoveryContestable,
        EdgeCode::IdentifierTaken,
        EdgeCode::LastAuthenticator,
        EdgeCode::OriginatingAdmin,
        EdgeCode::MembershipsRemain,
        EdgeCode::DelegationExpired,
        EdgeCode::DelegationRedeemed,
        EdgeCode::DelegationLimit,
        EdgeCode::LinkLocked,
        EdgeCode::TooManyAttempts,
        EdgeCode::AuthUnconfigured,
        EdgeCode::CredentialStoreUnavailable,
        EdgeCode::SessionStoreUnavailable,
        EdgeCode::DelegationStoreUnavailable,
        EdgeCode::LinkUnavailable,
        EdgeCode::TicketsUnavailable,
        EdgeCode::ShareUnavailable,
        EdgeCode::IdentityProviderUnavailable,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EdgeCode::InvalidRequest => "invalid_request",
            EdgeCode::TenantProvidersOff => "tenant_providers_off",
            EdgeCode::PresenceNotAvailable => "presence_not_available",
            EdgeCode::AuthenticationFailed => "authentication_failed",
            EdgeCode::RememberOff => "remember_off",
            EdgeCode::LinkPasswordRequired => "link_password_required",
            EdgeCode::LinkPasswordWrong => "link_password_wrong",
            EdgeCode::PasswordChangeRequired => "password_change_required",
            EdgeCode::ReauthenticationRequired => "reauthentication_required",
            EdgeCode::PresenceRequired => "presence_required",
            EdgeCode::DelegationRefused => "delegation_refused",
            EdgeCode::DelegationDeclined => "delegation_declined",
            EdgeCode::LinkRefused => "link_refused",
            EdgeCode::LinkingOff => "linking_off",
            EdgeCode::EmailUnavailable => "email_unavailable",
            EdgeCode::GuestsOff => "guests_off",
            EdgeCode::CodesDisabled => "codes_disabled",
            EdgeCode::ShareRefused => "share_refused",
            EdgeCode::InvitationRefused => "invitation_refused",
            EdgeCode::RecoveryContestable => "recovery_contestable",
            EdgeCode::IdentifierTaken => "identifier_taken",
            EdgeCode::LastAuthenticator => "last_authenticator",
            EdgeCode::OriginatingAdmin => "originating_admin",
            EdgeCode::MembershipsRemain => "memberships_remain",
            EdgeCode::DelegationExpired => "delegation_expired",
            EdgeCode::DelegationRedeemed => "delegation_redeemed",
            EdgeCode::DelegationLimit => "delegation_limit",
            EdgeCode::LinkLocked => "link_locked",
            EdgeCode::TooManyAttempts => "too_many_attempts",
            EdgeCode::AuthUnconfigured => "auth_unconfigured",
            EdgeCode::CredentialStoreUnavailable => "credential_store_unavailable",
            EdgeCode::SessionStoreUnavailable => "session_store_unavailable",
            EdgeCode::DelegationStoreUnavailable => "delegation_store_unavailable",
            EdgeCode::LinkUnavailable => "link_unavailable",
            EdgeCode::TicketsUnavailable => "tickets_unavailable",
            EdgeCode::ShareUnavailable => "share_unavailable",
            EdgeCode::IdentityProviderUnavailable => "identity_provider_unavailable",
        }
    }

    pub fn status(self) -> StatusCode {
        match self {
            EdgeCode::InvalidRequest
            | EdgeCode::TenantProvidersOff
            | EdgeCode::PresenceNotAvailable => StatusCode::BAD_REQUEST,
            EdgeCode::AuthenticationFailed
            | EdgeCode::RememberOff
            | EdgeCode::LinkPasswordRequired
            | EdgeCode::LinkPasswordWrong => StatusCode::UNAUTHORIZED,
            EdgeCode::PasswordChangeRequired
            | EdgeCode::ReauthenticationRequired
            | EdgeCode::PresenceRequired
            | EdgeCode::DelegationRefused
            | EdgeCode::DelegationDeclined
            | EdgeCode::LinkRefused
            | EdgeCode::LinkingOff
            | EdgeCode::EmailUnavailable
            | EdgeCode::GuestsOff
            | EdgeCode::CodesDisabled
            | EdgeCode::ShareRefused
            | EdgeCode::InvitationRefused => StatusCode::FORBIDDEN,
            EdgeCode::RecoveryContestable
            | EdgeCode::IdentifierTaken
            | EdgeCode::LastAuthenticator
            | EdgeCode::OriginatingAdmin
            | EdgeCode::MembershipsRemain => StatusCode::CONFLICT,
            EdgeCode::DelegationExpired | EdgeCode::DelegationRedeemed => StatusCode::GONE,
            EdgeCode::DelegationLimit | EdgeCode::LinkLocked | EdgeCode::TooManyAttempts => {
                StatusCode::TOO_MANY_REQUESTS
            }
            EdgeCode::AuthUnconfigured
            | EdgeCode::CredentialStoreUnavailable
            | EdgeCode::SessionStoreUnavailable
            | EdgeCode::DelegationStoreUnavailable
            | EdgeCode::LinkUnavailable
            | EdgeCode::TicketsUnavailable
            | EdgeCode::ShareUnavailable
            | EdgeCode::IdentityProviderUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// The word for a refusal named at run time (a ticket kind's own
    /// refusal), if it is one of this inventory's.
    pub fn from_word(word: &str) -> Option<EdgeCode> {
        EdgeCode::ALL.iter().copied().find(|c| c.as_str() == word)
    }
}

/// A refusal from one of the surfaces [`EdgeCode`] names. The status is the
/// code's; the handler supplies the sentence and any named fields (the
/// setting that fixes it, the case to contest, the seconds to wait).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeError {
    code: EdgeCode,
    message: String,
    /// Fields inside `error`, after `code` and `message`.
    fields: Map<String, Value>,
    /// Fields beside `error` at the top of the body.
    beside: Map<String, Value>,
    retry_after: Option<u64>,
}

impl EdgeError {
    pub fn new(code: EdgeCode, message: impl Into<String>) -> EdgeError {
        EdgeError {
            code,
            message: message.into(),
            fields: Map::new(),
            beside: Map::new(),
            retry_after: None,
        }
    }

    /// A named field inside the `error` object (`setting`, `connection`).
    pub fn with(mut self, key: &str, value: impl serde::Serialize) -> EdgeError {
        self.fields.insert(key.to_string(), to_value(value));
        self
    }

    /// A field beside the `error` object, at the top of the body.
    pub fn beside(mut self, key: &str, value: impl serde::Serialize) -> EdgeError {
        self.beside.insert(key.to_string(), to_value(value));
        self
    }

    /// A `Retry-After` header, in seconds.
    pub fn retry_after(mut self, secs: u64) -> EdgeError {
        self.retry_after = Some(secs);
        self
    }

    pub fn code(&self) -> EdgeCode {
        self.code
    }

    pub fn envelope(&self) -> Value {
        let mut inner = Map::new();
        inner.insert("code".into(), Value::String(self.code.as_str().into()));
        inner.insert(
            "message".into(),
            Value::String(caller_safe(&self.message).into_owned()),
        );
        for (k, v) in &self.fields {
            inner.insert(k.clone(), v.clone());
        }
        let mut outer = self.beside.clone();
        outer.insert("error".into(), Value::Object(inner));
        Value::Object(outer)
    }
}

/// A named field's value. A value that does not serialise is `null`, never
/// a panic on the error path.
fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

impl IntoResponse for EdgeError {
    fn into_response(self) -> Response {
        let mut response = (self.code.status(), Json(self.envelope())).into_response();
        retry_after(&mut response, self.retry_after);
        response
    }
}

/// A status report whose body is a state, not an error: `200` when the
/// thing reported is ready, `503` when it is not (`/health/ready`,
/// `/auth/provisioning`). A load balancer reads the status; a person reads
/// the body.
pub fn status_report(ready: bool, body: Value) -> Response {
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

/// A document this deployment does not publish: `404` with no body, because
/// there is nothing to say (`/.well-known/webauthn` with no related origins).
pub fn no_document() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No envelope carries the database's words, whatever its class: every
    /// op class and every edge class, given a message with the pool's
    /// `db:` text, answers a body without it.
    #[test]
    fn no_error_body_carries_database_words() {
        let leaks = [
            "db: ERROR: invalid regular expression: invalid backreference number",
            "space_unavailable: db: ERROR: relation \"x\" does not exist",
            "ERROR: canceling statement due to statement timeout",
        ];
        for m in leaks {
            for &code in ErrorCode::ALL {
                let body = ApiError::new(code, m).envelope().to_string();
                assert!(!body.contains("db:") && !body.contains("ERROR:"), "{body}");
            }
            for &code in EdgeCode::ALL {
                let body = EdgeError::new(code, m).envelope().to_string();
                assert!(!body.contains("db:") && !body.contains("ERROR:"), "{body}");
            }
        }
        let fine = ApiError::new(ErrorCode::InvalidParam, "the pattern is not valid").envelope();
        assert_eq!(fine["error"]["message"], "the pattern is not valid");
    }

    /// `as_str` is exhaustive by construction — the compiler will not let a
    /// new variant skip it. [`ErrorCode::ALL`] is not, and it is what
    /// `from_class` scans, so a code missing from it would be a class the
    /// tiers below the transport can raise and the transport cannot read
    /// back. This is the tripwire for that.
    #[test]
    fn every_code_round_trips_through_its_class_name() {
        for &code in ErrorCode::ALL {
            assert_eq!(
                ErrorCode::from_class(code.as_str()),
                Some(code),
                "`{}` is in ALL but does not read back",
                code.as_str()
            );
        }
    }

    /// The names are the wire contract and the docs' closed inventory
    /// (`writing-data.md` §7, `api-engine-design.md` §4). No duplicates,
    /// and every one in `snake_case` — a class that renames itself breaks
    /// every client that switched on it.
    #[test]
    fn class_names_are_distinct_and_well_formed() {
        let mut seen: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), count, "two codes share a class name");
        for name in seen {
            assert!(
                !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                    && !name.starts_with('_')
                    && !name.ends_with('_'),
                "`{name}` is not a snake_case class name"
            );
        }
    }

    /// The door's words are the door's alone (§4.1 non-enumerability): an
    /// executor that answered one would be telling the caller which layer
    /// refused them.
    #[test]
    fn the_executor_may_not_borrow_the_doors_answer() {
        for &code in &[
            ErrorCode::AccessDenied,
            ErrorCode::CallerSupplied,
            ErrorCode::UnknownOp,
            ErrorCode::MissingSpace,
            ErrorCode::IdempotencyInFlight,
            ErrorCode::IdempotencyApplied,
        ] {
            assert!(!code.executor_raisable(), "{}", code.as_str());
            let spec = OpSpec {
                name: "op".into(),
                version_hash: "v".into(),
                kind: crate::spec::OpKind::Write,
                params: Vec::new(),
                output_fields: Vec::new(),
                live_capable: false,
                sync: crate::spec::Syncability::NotSyncable(crate::spec::NotSyncable::NotARead),
                requires_idempotency_key: false,
                public: Default::default(),
                refinable: crate::spec::RefineTier::None,
                aggregate_only: false,
                mode_label: None,
                cost: crate::spec::OpCost::default(),
            };
            let downgraded = ApiError::from_exec(&spec, ExecError::new(code, "x"));
            assert_eq!(downgraded.code, ErrorCode::Internal);
        }
    }

    /// Every class the write path raises answers in a family a caller can
    /// act on. A typed refusal that lands in 5xx is the 9g bug restated.
    #[test]
    fn write_path_refusals_are_client_answerable() {
        for &code in &[
            ErrorCode::HasReferrers,
            ErrorCode::TreeCycle,
            ErrorCode::TreeHasChildren,
            ErrorCode::TreeDepthExceeded,
            ErrorCode::TreeBulkReparent,
            ErrorCode::ReservedDerivedName,
            ErrorCode::AmbiguousIdentity,
            ErrorCode::UnboundRequiredParam,
            ErrorCode::ProjectionUnknownField,
            ErrorCode::InvalidBindingExpr,
            ErrorCode::ParamTypeMismatch,
            ErrorCode::NullKeyAddress,
            ErrorCode::AccessGuard,
            ErrorCode::NotFound,
        ] {
            assert!(
                code.status().is_client_error(),
                "`{}` answers {} — a built rule reads as a server fault",
                code.as_str(),
                code.status()
            );
        }
    }

    /// Ruled 2026-10-05 (`audit-2026-10-05c` ask 2). Sealed reads are
    /// served, so what is left is an op shape the app developer fixes.
    #[test]
    fn sealed_unsupported_answers_422_as_an_op_the_app_developer_fixes() {
        let code = ErrorCode::SealedUnsupported;
        assert_eq!(code.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(code.status().is_client_error());
    }

    fn spec() -> OpSpec {
        OpSpec {
            name: "op".into(),
            version_hash: "v".into(),
            kind: crate::spec::OpKind::Read,
            params: Vec::new(),
            output_fields: Vec::new(),
            live_capable: false,
            sync: crate::spec::Syncability::NotSyncable(crate::spec::NotSyncable::NotARead),
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: crate::spec::RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: crate::spec::OpCost::default(),
        }
    }

    /// 27ar. A paused tenant answers 423: nothing a retry fixes, nothing
    /// the caller lacks, and never a refusal a replay would hand back once
    /// the tenant is resumed.
    #[test]
    fn space_paused_is_a_423_the_executor_may_raise_and_replay_never_caches() {
        let code = ErrorCode::SpacePaused;
        assert_eq!(code.as_str(), "space_paused");
        assert_eq!(ErrorCode::from_class("space_paused"), Some(code));
        assert_eq!(code.status(), StatusCode::LOCKED);
        assert!(code.executor_raisable());
        assert!(!code.is_exec_conflict());
        let e = ApiError::from_exec(&spec(), ExecError::new(code, "paused"));
        assert_eq!(e.code, ErrorCode::SpacePaused);
        assert_eq!(e.into_response().status(), StatusCode::LOCKED);
    }

    /// The class is `space_paused` whatever status the app developer set,
    /// and a 503 is still never cached for replay.
    #[test]
    fn space_paused_keeps_its_class_whatever_the_status() {
        for status in [400u16, 402, 403, 423, 451, 503] {
            let mut exec = ExecError::new(ErrorCode::SpacePaused, "paused");
            exec.status = Some(status);
            let e = ApiError::from_exec(&spec(), exec);
            assert_eq!(e.code, ErrorCode::SpacePaused);
            assert_eq!(e.envelope()["error"]["code"], "space_paused");
            assert!(!e.code.is_exec_conflict(), "never cached for replay");
            assert_eq!(e.into_response().status().as_u16(), status);
        }
        // A status outside the allowed range is ignored, and so is one on
        // any other class.
        let mut exec = ExecError::new(ErrorCode::SpacePaused, "paused");
        exec.status = Some(200);
        let e = ApiError::from_exec(&spec(), exec);
        assert_eq!(e.into_response().status(), StatusCode::LOCKED);
        let mut exec = ExecError::new(ErrorCode::Internal, "x");
        exec.status = Some(402);
        let e = ApiError::from_exec(&spec(), exec);
        assert_eq!(
            e.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    /// Reported fix 25z. A full pool is a 503 the client retries, never a
    /// 500, and never a refusal a replay would hand back.
    #[test]
    fn server_busy_is_a_retryable_503_with_retry_after() {
        let code = ErrorCode::ServerBusy;
        assert_eq!(code.as_str(), "server_busy");
        assert_eq!(code.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(code.executor_raisable());
        assert!(!code.is_exec_conflict());
        let response = ApiError::server_busy().into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::RETRY_AFTER).unwrap(),
            SERVER_BUSY_RETRY_AFTER
        );
        let other = ApiError::new(ErrorCode::SpaceUnavailable, "x").into_response();
        assert!(other.headers().get(header::RETRY_AFTER).is_none());
    }

    /// A busy fault reaches the door as `server_busy` by its type, whatever
    /// class the tier would have filed anything else under. The door never
    /// reads the words: an `internal` that quotes them stays `internal`.
    #[test]
    fn a_busy_fault_reaches_the_door_as_server_busy_by_type() {
        for otherwise in [ErrorCode::Internal, ErrorCode::SpaceUnavailable] {
            let exec = ExecError::of_fault(FaultClass::Busy, otherwise, "store: down");
            let e = ApiError::from_exec(&spec(), exec);
            assert_eq!(e.code, ErrorCode::ServerBusy, "{otherwise:?}");
            assert_eq!(e.message, server_busy_message());
            let exec = ExecError::of_fault(FaultClass::Broken, otherwise, "store: down");
            let e = ApiError::from_exec(&spec(), exec);
            assert_eq!((e.code, e.message.as_str()), (otherwise, "store: down"));
        }
        let e = ApiError::from_exec(&spec(), ExecError::internal(SERVER_BUSY_MESSAGE));
        assert_eq!(e.code, ErrorCode::Internal, "the words decide nothing");
    }

    /// Owner, 2026-10-23: an unreadable fault reaches the door as its own
    /// non-retryable code, whatever the op would otherwise have said.
    #[test]
    fn an_unreadable_fault_reaches_the_door_as_sealed_unreadable() {
        let exec = ExecError::of_fault(FaultClass::Unreadable, ErrorCode::Internal, "x.y");
        assert_eq!(exec.code, ErrorCode::SealedUnreadable);
        assert!(exec.code.executor_raisable());
        assert!(!exec.code.is_exec_conflict());
        assert_eq!(ErrorCode::SealedUnreadable.as_str(), "sealed_unreadable");
        assert_eq!(
            ErrorCode::SealedUnreadable.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn by_fault_answers_busy_for_a_retryable_class_and_broken_otherwise() {
        let busy = || ApiError::server_busy();
        let broken = || ApiError::new(ErrorCode::Internal, "blob storage");
        for (class, want) in [
            (FaultClass::Busy, ErrorCode::ServerBusy),
            (FaultClass::Retryable, ErrorCode::ServerBusy),
            (FaultClass::Refused, ErrorCode::Internal),
            (FaultClass::Broken, ErrorCode::Internal),
        ] {
            assert_eq!(ApiError::by_fault(class, busy(), broken()).code, want);
        }
    }

    /// The edge's other inventory: distinct snake_case words, each with a
    /// status in the error families, and the envelope the handlers built by
    /// hand, byte for byte.
    #[test]
    fn edge_codes_are_distinct_and_answer_an_error_status() {
        let mut seen: Vec<&str> = EdgeCode::ALL.iter().map(|c| c.as_str()).collect();
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), count, "two edge codes share a word");
        for &code in EdgeCode::ALL {
            assert_eq!(EdgeCode::from_word(code.as_str()), Some(code));
            assert!(
                code.status().is_client_error() || code.status().is_server_error(),
                "{}",
                code.as_str()
            );
            // One word means the same on both surfaces, by design: a
            // passkey touch newer than the one held.
            if code == EdgeCode::PresenceRequired {
                assert_eq!(
                    ErrorCode::from_class(code.as_str()),
                    Some(ErrorCode::PresenceRequired)
                );
                assert_eq!(ErrorCode::PresenceRequired.status(), code.status());
                continue;
            }
            assert!(
                ErrorCode::from_class(code.as_str()).is_none(),
                "`{}` is an op class too",
                code.as_str()
            );
        }
    }

    #[test]
    fn an_edge_error_keeps_its_fields_and_retry_after() {
        let e = EdgeError::new(EdgeCode::LinkLocked, "wait")
            .with("retry_after", 7)
            .with("setting", "S")
            .beside("tenants", serde_json::json!(["t"]))
            .retry_after(7);
        assert_eq!(
            e.envelope(),
            serde_json::json!({
                "error": {"code": "link_locked", "message": "wait",
                          "retry_after": 7, "setting": "S"},
                "tenants": ["t"],
            })
        );
        let response = e.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "7");
    }
}
