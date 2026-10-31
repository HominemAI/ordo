//! The write protocol, executed by the harness (Stage 5).
//!
//! This is the conformance stand-in for the F5 executor, with the same
//! honesty rule as the read side: everything here IS the semantics — mode
//! derivation, guard preconditions, rev bumps, typed-id minting, and the
//! command-group append — but the plumbing is psql round-trips. Three gaps
//! are the executor's to close, not ours to fake (AA-004): precondition
//! checks and the mutation batch are not one snapshot (no SELECT FOR
//! UPDATE); the log append is in-memory, not the durable chain; and
//! tree/sealed writes are refused rather than staged.
//!
//! The protocol per statement:
//!
//! - **create** (`set` with no condition, no identity in the braces): every
//!   supplied ref must name a row the caller can READ (G12 — an invisible
//!   parent is `not_found`, indistinguishable from absent); the write guard
//!   must hold against the would-be row unless it is create-exempt (G10a);
//!   ids are minted from the entity's key; `rev` starts at 1; an acl root
//!   gets its creator's top-level access row in the same command group (the
//!   bootstrap).
//! - **converge-or-create** (`set` with no condition whose braces cover
//!   exactly one unique key — §6 rule 8, the one blessed upsert): the key
//!   addresses, so the statement converges onto the row holding it and
//!   creates when no row does. `INSERT … ON CONFLICT` (`implementation.md`
//!   pins it) makes that decision the database's, in one statement, rather
//!   than a read followed by a write with a race between them.
//! - **addressed** (`set`/`del` on `id == …`): outside the read view is
//!   `not_found`; visible but failing the write guard is `access_guard`;
//!   then UPDATE bumps `rev`, images before and after.
//! - **bulk** (any other condition): the selection is read-view ∩ cond ∩
//!   write-view — rows the caller cannot write are silently outside the
//!   range (G11), never an error, exactly like rows a guard hides from a
//!   read.

use std::collections::BTreeMap;

use ordo_commandlog::store::Store;
use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Image, InMemoryStore,
    SpaceId, Timestamp, Value as LogValue,
};
use ordo_plan::plan;
use ordo_schema::{Entity, FieldTy};
use ordo_sema::{
    bind_as, create_write_query, fold_neg_lit, guard_filter_query, CreateGate, CreateVal,
};
use ordo_syntax::{
    parse_pipeline, Condition, Expr, OpBody, Pipeline, ProjItem, ScalarTy, Source, SourceArg, Span,
    WriteStmt,
};
use serde_json::{json, Value};

use crate::{Fixture, Pg};

/// The platform's tree depth cap (§tree, settled: "enforces a depth cap").
/// Deep enough for any hierarchy a human designs, shallow enough that a
/// path column and a runaway reparent loop both stay bounded.
pub const TREE_DEPTH_CAP: i64 = 64;

/// **The one `not_found`** — every producer of the class emits this exact
/// string and nothing else.
///
/// R-G10 says a row outside the caller's read view answers *byte-identical*
/// to a row that never existed: "same status, same code, same message".
/// That is a property of the whole class, not of any one call site, so the
/// class gets one message rather than a family of messages that happen to
/// agree today. The message the platform used to emit — ``not_found: `x`
/// (tasks)`` — echoed the caller's own id, which discloses nothing, but it
/// also named the ENTITY, and that is not always the caller's own input: a
/// statement that can fail `not_found` in two places (the addressed row and
/// a supplied parent) told the caller *which* of the two was unreachable,
/// and therefore that the other one was fine. That is an existence answer
/// about a row, reached without ever seeing it.
///
/// Where the id should live instead, stated honestly. R-G10 says "the wire
/// is uniform; the record is not", and the *distinction* does survive in the
/// record: `ordo_secevent::RecordedDecision` has both `GuardRejected` and
/// `RowAbsent`, and they are separate variants precisely because the wire
/// erases the difference. What does **not** survive anywhere today is the
/// addressed id itself — `StreamEntry` carries scope, subject, op, decision
/// and a bounded sample of correlation ids, and is documented as
/// deliberately payload-free. So an operator can see *that* a caller was
/// refused and *why*; they cannot see *which row*.
///
/// That is the right destination and the wrong current state: the id belongs
/// in the space's own record, correlated to the request, not in the caller's
/// error message where it is one half of an enumeration answer. Adding it is
/// owed work on `security-logging-design.md`'s surface, not a reason to keep
/// it on the wire — a refusal that discloses is not made safe by being
/// useful.
///
/// The colon is load-bearing: `ordo_host::classify` splits `class: message`
/// on the first one, and a message without it decodes as `internal`.
pub const NOT_FOUND: &str = "not_found: no such row";

/// `NOT_FOUND` as an `ok_or_else` argument.
fn not_found() -> String {
    NOT_FOUND.to_string()
}

/// The write path's only contact with the database: one text-protocol
/// query. Implemented by the psql harness ([`Pg`]) and — the point of the
/// seam — by an open transaction on a pooled connection, which is how the
/// whole protocol (preconditions, locks, mutations) runs under ONE
/// snapshot instead of a sequence of independent statements (AA-004).
pub trait WriteDb {
    fn rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String>;

    /// Whether a statement this connection ran was refused by a foreign key
    /// or a unique constraint. The error channel above is text, so this is
    /// how that class survives it: read from the database client's typed
    /// error (`PoolError::is_foreign_key_violation`), never from the words.
    /// A connection that cannot tell answers `false`.
    fn refused_by_constraint(&self) -> bool {
        false
    }
}

impl WriteDb for Pg {
    fn rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        self.query(sql)
    }
}

/// A value the **platform** produced, already bound to the one row it may
/// be written to, with no source expression behind it.
///
/// Every ordinary right-hand side reaches SQL through `eval_expr`, which
/// starts from an `Expr` an op author wrote. A blob upload has none and
/// must not have one: `Sema::check_content_params` refuses any op parameter
/// whose type is content, because a caller-supplied handle names an object
/// the caller did not upload. So the blob door's write cannot be spelled as
/// a call, and this is the shape it arrives in instead. It is the seam
/// `docs/plan-blob-chain.md` §1 designs.
///
/// **It names its own target**, so [`Fixture::run_platform_converge`] reads
/// entity, field and row id out of the value rather than from arguments
/// beside it: there is no call shape in which a value is applied to a row
/// it was not minted for. **It carries its own reference statement**, so
/// the door issues no SQL at all and the `$blobref` insert and the column
/// write are one thing the protocol executes in one order.
///
/// # Why a trait, and what it does not buy
///
/// The one implementor lives in `ordo-host`, three crates above this one,
/// and a concrete type cannot travel downwards. Nor can this crate reach
/// up: `ordo-wasm` compiles `ordo-conformance` for
/// `wasm32-unknown-unknown`, and `ordo-blob` pulls tokio with `fs` and
/// `net`, so the dependency that would let the concrete token be named here
/// is one the browser build cannot have.
///
/// What the token itself gets is the whole type-level pin: private fields,
/// one private constructor, no `Clone`, no serde, consumed by move. What
/// the trait cannot get is "nothing else implements it", because sealing
/// it would exclude `ordo-host` too. That residue is closed the way this corpus
/// already closes this class of thing, with a source scan:
/// `the_platform_value_trait_has_one_implementor`.
pub trait PlatformValue {
    /// Entity, field, row id. The protocol's only source for all three.
    fn target(&self) -> (&str, &str, &str);

    /// The right-hand side of the column's `SET`, already a SQL literal.
    fn column_literal(&self) -> String;

    /// What that column reads as in a command image, derived by the
    /// producer rather than by re-decoding the row. The protocol images the
    /// row itself and compares, so the two are independent answers to one
    /// question.
    fn image_value(&self) -> LogValue;

    /// The `$blobref` `INSERT` that must land before the column names it.
    fn reference_sql(&self) -> String;
}

/// One effect of a leave, as the door resolved it from the program's
/// `release member` block (checklist 27aq). See [`Fixture::run_release_on`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseStep {
    /// Delete the rows of `entity` whose `field` is the seat.
    Remove { entity: String, field: String },
    /// Point `field` of the rows of `entity` that name the seat at `to`, a
    /// member the door chose.
    Reassign {
        entity: String,
        field: String,
        to: String,
    },
}

/// **The write path's one choke point, and what makes an unscoped write
/// statement unshippable.**
///
/// The read side gets this structurally: the planner emits the space
/// predicate as part of rendering a source, so there is nothing to forget.
/// The write side builds SQL by hand in seventeen places, and "remember to
/// add the predicate in seventeen places" is exactly the shape of rule that
/// fails on the eighteenth. So every statement the protocol issues is
/// wrapped here on its way to the database, and one that names a table
/// without confining it is a hard error instead of a cross-tenant write.
///
/// `run_write_on` wraps its `db` once, at the entry point, so this covers
/// preconditions, probes, locks, images, mutations and the spliced-in read
/// plans alike — including any statement added later by someone who never
/// read this comment.
pub(crate) struct SpaceAudit<'a> {
    pub(crate) inner: &'a dyn WriteDb,
    pub(crate) space: String,
}

impl WriteDb for SpaceAudit<'_> {
    fn rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        audit_scoped(sql, &self.space)?;
        self.inner.rows(sql)
    }

    fn refused_by_constraint(&self) -> bool {
        self.inner.refused_by_constraint()
    }
}

/// How many base tables a statement names, counted at the four places SQL
/// can name one.
fn table_mentions(sql: &str) -> usize {
    let gross: usize = ["FROM \"", "INTO \"", "UPDATE \"", "JOIN \""]
        .iter()
        .map(|k| sql.matches(k).count())
        .sum();
    // `IS [NOT] DISTINCT FROM "col"` also contains `FROM "`, and it is a
    // COMPARISON, not a table mention. Two-valued logic made every equality
    // compile to that form, so without this subtraction the audit counts one
    // phantom table per comparison and refuses statements it should pass —
    // which is exactly how it failed the day the distinctness family landed.
    //
    // Subtracting rather than switching to a parser on purpose: the audit is
    // a blunt instrument by design, and it fails CLOSED. A miscount can only
    // ever refuse a good statement, never admit an unscoped one.
    gross - sql.matches("DISTINCT FROM \"").count()
}

/// Every table a statement names must be confined, and confined to THIS
/// space.
///
/// Two independent failures, both real: a statement with no `$space` at all
/// reads every tenant's rows, and a statement carrying some *other* space's
/// literal reads the wrong tenant's. The second is the one a copy-paste
/// produces, so it is checked rather than assumed away.
fn audit_scoped(sql: &str, space: &str) -> Result<(), String> {
    let tables = table_mentions(sql);
    if tables == 0 {
        return Ok(());
    }
    let col = q_ident(ordo_schema::SPACE_COLUMN);
    if sql.matches(&col).count() < tables {
        return Err(format!(
            "unscoped_write: this statement names {tables} table{} and constrains {} of them by \
             space — every statement the write path issues is confined to one space:\n{sql}",
            if tables == 1 { "" } else { "s" },
            sql.matches(&col).count(),
        ));
    }
    if !sql.contains(&sql_str(space)) {
        return Err(format!(
            "unscoped_write: this statement is scoped, but not to `{space}` — the space a write \
             runs in is the one the door bound, never one a statement chose:\n{sql}"
        ));
    }
    Ok(())
}

/// A sealed payload minted for an image, waiting for the group it belongs
/// to to have a correlation id.
///
/// The correlation is minted **last** (it counts after every command id and
/// timestamp), and payload rows record it so a prune can collect what it
/// orphans — so payloads cannot be written the moment they are minted. They
/// are staged here instead and flushed in one pass at the end of the write,
/// inside the same transaction as the rows they describe. The ordering is
/// deliberate in the other direction too: a write that fails while planning
/// stages payloads and never flushes them, so a refused write leaves no
/// payload behind even on the harness's non-transactional `psql` path.
struct PendingPayload {
    handle: ordo_crypto::SealedHandle,
    epoch: ordo_crypto::Epoch,
    entity: String,
    field: String,
    /// The label of the anchor the envelope was sealed at: the row it is a
    /// copy of the envelope of.
    anchor: String,
    bytes: Vec<u8>,
}

/// The per-suite write log: the in-memory chain plus the counters (id
/// minting, correlation, timestamps) a write stamps with.
///
/// A suite's log mints from one deterministic counter, which is what keeps
/// runs replayable. A deployment's log mints differently
/// ([`WriteLog::serving`]), because a deployment restarts and a suite does
/// not.
pub struct WriteLog {
    pub store: InMemoryStore,
    /// Shared with every [`WriteLog::attempt`] cut from this log, so an
    /// attempt mints from the same sequence as the log it will commit into.
    seq: std::sync::Arc<std::sync::atomic::AtomicU64>,
    minting: Minting,
    /// The group this write built, held back until its transaction
    /// commits. See the comment at the end of `run_write_on`.
    pending: Option<CommandGroup>,
    /// Sealed payloads staged by the write in flight — see
    /// [`PendingPayload`].
    payloads: Vec<PendingPayload>,
    /// The **batch correlation**, when a bulk import has one open.
    ///
    /// Set by [`WriteLog::open_batch`]. While it is set, every call run
    /// against this log builds its group under *this* correlation and
    /// **extends** the pending group instead of replacing it, so a batch of
    /// rows becomes one command group and one chain entry rather than one
    /// each.
    ///
    /// The shared correlation is load-bearing rather than cosmetic.
    /// `$sealedpayload` rows are stamped with the correlation of the group
    /// whose image minted them, and collection is the anti-join *"payload
    /// rows in this space whose correlation is no longer in the chain"*
    /// (`ordo_crypto::sealed`). A merged group carrying a correlation the
    /// payloads beneath it were never stamped with would leave every sealed
    /// value in the batch collectable.
    batch: Option<CorrelationId>,
    /// The agent this attempt's write is made by, when it acts for the
    /// caller under a delegation. Set by [`WriteLog::act_for`] on an
    /// attempt, never inherited: each attempt is one request's.
    agent: Option<String>,
    /// The routine this attempt's write is made by (checklist 12a), as
    /// `<name>@<version>`. Set by [`WriteLog::act_as_routine`] on an
    /// attempt, never inherited. When set, the group names
    /// `routine:<name>@<version>` and no user (DL §8, "Its attribution is
    /// its third [constraint]").
    routine: Option<String>,
}

/// How a [`WriteLog`] mints the ids and times a write stamps.
#[derive(Clone)]
enum Minting {
    /// One counter for ids and times alike. Deterministic, so a suite
    /// replays byte for byte, and unique only for the life of one log,
    /// which is all a suite needs.
    Counter,
    /// For a log serving a deployment. Ids carry a prefix the caller chose
    /// to be unique across processes, and times come from a clock.
    Serving {
        prefix: String,
        clock: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>,
    },
}

impl WriteLog {
    pub fn new() -> WriteLog {
        WriteLog {
            store: InMemoryStore::new(),
            seq: Default::default(),
            minting: Minting::Counter,
            pending: None,
            payloads: Vec::new(),
            batch: None,
            agent: None,
            routine: None,
        }
    }

    /// A log for a deployment rather than a suite.
    ///
    /// A counter that restarts with the process mints `cg_1` again for a
    /// space whose durable chain already holds it, which the chain refuses
    /// as a duplicate correlation, and mints a row id a table already
    /// holds. So every id this log mints carries `prefix`, which the caller
    /// draws to be unique per log, and every command is stamped with
    /// `clock`, in the seconds the undo horizon compares against.
    pub fn serving(
        prefix: impl Into<String>,
        clock: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> WriteLog {
        WriteLog {
            minting: Minting::Serving {
                prefix: prefix.into(),
                clock,
            },
            ..WriteLog::new()
        }
    }

    fn next(&mut self) -> u64 {
        self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
    }

    /// A log for one write attempt: it mints from this log's sequence and
    /// with its minting, and holds its own pending group and staged
    /// payloads, starting empty.
    ///
    /// This is what lets concurrent writers share a log without holding it
    /// across database I/O. The attempt runs the whole write protocol
    /// against its own log, so no lock is held while a statement waits on a
    /// row lock, and one writer's pending group cannot be dropped or taken
    /// by another. The shared log is touched again only by
    /// [`WriteLog::commit_attempt`], after the transaction commits.
    pub fn attempt(&self) -> WriteLog {
        WriteLog {
            store: InMemoryStore::new(),
            seq: self.seq.clone(),
            minting: self.minting.clone(),
            pending: None,
            payloads: Vec::new(),
            // An attempt inherits the open batch: the batch's calls run on
            // the attempt's log, so the correlation has to be there and not
            // on the shared log they never touch.
            batch: self.batch.clone(),
            // One request's agent. An attempt cut for another request must
            // not carry it, so it starts empty and the executor sets it.
            agent: None,
            routine: None,
        }
    }

    /// Record this attempt's writes as made by `agent` **on behalf of** the
    /// caller, for a call the door admitted under a delegation.
    ///
    /// The caller stays the guard input (`$caller` is the delegator,
    /// `plan-delegation-surface.md` §7); this only changes who the command
    /// group names. `None` is an ordinary call.
    pub fn act_for(&mut self, agent: Option<String>) {
        self.agent = agent;
    }

    /// Record this attempt's writes as made by a routine, `<name>@<version>`
    /// (checklist 12a). The command group names `routine:<name>@<version>`,
    /// which is the producer `Actor::Routine` had been waiting for (12e).
    pub fn act_as_routine(&mut self, routine: Option<String>) {
        self.routine = routine;
    }

    /// The actor a group built by this log names for `caller`:
    /// `agent:<agent> on behalf of user:<caller>` under a delegation,
    /// `user:<caller>` otherwise. The one place an op write's actor is
    /// built, so the two forms cannot drift apart.
    pub fn actor_for(&self, caller: &str) -> Actor {
        if let Some(routine) = &self.routine {
            return Actor::Routine(routine.clone());
        }
        match &self.agent {
            Some(agent) => Actor::AgentOnBehalfOf {
                agent: agent.clone(),
                user: caller.to_string(),
            },
            None => Actor::User(caller.to_string()),
        }
    }

    /// Append the group an [`WriteLog::attempt`] built, once its
    /// transaction has committed.
    pub fn commit_attempt(&mut self, attempt: &mut WriteLog) -> Result<(), String> {
        let Some(group) = attempt.pending.take() else {
            return Ok(());
        };
        self.store
            .append(group)
            .map(|_| ())
            .map_err(|e| format!("log append: {e:?}"))
    }

    /// Open a batch: every call run against this log until
    /// [`WriteLog::close_batch`] joins one command group under
    /// `correlation`.
    ///
    /// Two writers: `ordo_exec::import`, and a leave
    /// ([`Fixture::run_release_on`]), whose effects, membership removals
    /// and seat are one group. See the [`WriteLog::batch`] field
    /// for why the correlation is shared rather than merged afterwards.
    pub fn open_batch(&mut self, correlation: impl Into<String>) {
        self.batch = Some(CorrelationId(correlation.into()));
    }

    /// Close the batch. The pending group, if any, is untouched: the
    /// caller still appends it, exactly as a single write's caller does.
    pub fn close_batch(&mut self) {
        self.batch = None;
    }

    /// Is a batch open on this log?
    pub fn in_batch(&self) -> bool {
        self.batch.is_some()
    }

    /// The correlation the group about to be built carries: the batch's
    /// when one is open, a freshly minted one otherwise.
    pub(crate) fn group_correlation(&mut self) -> CorrelationId {
        match &self.batch {
            Some(c) => c.clone(),
            None => CorrelationId(format!("cg_{}", self.mint())),
        }
    }

    /// Hold back the group this call built.
    ///
    /// Outside a batch this replaces the pending group, which is what a
    /// write has always done. Inside one it **extends** it, so N calls
    /// leave one group holding N commands.
    ///
    /// The two header fields are checked rather than assumed on the way
    /// in. A group carries one actor and one rule-6 flag for the whole of
    /// it, so a batch that changed either mid-way would produce a group
    /// attributing commands to a principal that did not issue them. That is
    /// a lie in the chain, and the chain's value is that a reader may take
    /// what it says literally.
    pub(crate) fn stage_group(&mut self, group: CommandGroup) -> Result<(), String> {
        match self.pending.as_mut() {
            Some(open) if self.batch.is_some() => {
                if open.actor != group.actor {
                    return Err(format!(
                        "batch_actor_changed: this batch's group is attributed to `{}`, \
                         this call to `{}` — one group carries one actor",
                        open.actor, group.actor
                    ));
                }
                if open.owner_administration != group.owner_administration {
                    return Err(
                        "batch_flag_changed: owner administration is a property of the whole \
                         group, so a batch may not mix rule-6 and ordinary calls"
                            .into(),
                    );
                }
                open.commands.extend(group.commands);
                Ok(())
            }
            _ => {
                self.pending = Some(group);
                Ok(())
            }
        }
    }

    /// The part of a minted id that makes it unique: the count alone for a
    /// suite, and for a deployment the time, the prefix and the count.
    ///
    /// A deployment's ids sort by age (25e), as `definition-language.md`
    /// §2 promises ("time-ordered"): the milliseconds since the epoch first,
    /// then the log's prefix, then the count, each at a fixed width. Ids
    /// minted in one log sort in the order they were minted, and an id
    /// minted a millisecond later in any log sorts after. Two logs in the
    /// same millisecond order by prefix, as two UUIDv7s in one millisecond
    /// order by their random bits.
    pub(crate) fn mint(&mut self) -> String {
        let n = self.next();
        match &self.minting {
            Minting::Counter => n.to_string(),
            Minting::Serving { prefix, .. } => {
                let ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                format!("{ms:012x}{prefix}{n:012x}")
            }
        }
    }

    /// The time a command is stamped with.
    pub(crate) fn stamp(&mut self) -> u64 {
        if let Minting::Serving { clock, .. } = &self.minting {
            return clock();
        }
        self.next()
    }

    /// Append the group the last write built, now that its transaction
    /// has committed. A caller that never reaches commit simply drops it.
    pub fn commit_pending(&mut self) -> Result<(), String> {
        let Some(group) = self.pending.take() else {
            return Ok(());
        };
        self.store
            .append(group)
            .map(|_| ())
            .map_err(|e| format!("log append: {e:?}"))
    }

    /// Discard a group whose transaction did not commit.
    pub fn drop_pending(&mut self) {
        self.pending = None;
    }

    /// The group this write built, still held back.
    ///
    /// A durable store appends **inside** the effect's transaction rather
    /// than after its commit, so it needs the group before `commit_pending`
    /// is reachable. Borrowed, not taken: the in-memory mirror still
    /// appends its own copy on commit, and a caller that could remove the
    /// group here would silently disarm `commit_pending`.
    pub fn pending(&self) -> Option<&CommandGroup> {
        self.pending.as_ref()
    }

    /// Groups appended to a space's chain so far, for chain assertions.
    ///
    /// The chain is per space (`implementation.md`: one ordering point per
    /// space), so counting it needs one — there is no total across the
    /// deployment to ask for, and a count that quietly summed two tenants'
    /// chains would be describing a chain that does not exist.
    pub fn len_in(&self, space: &SpaceId) -> usize {
        self.store.len(space)
    }

    /// The conformance space's chain length — the one every suite but
    /// `tests/spaces.rs` writes to.
    pub fn len(&self) -> usize {
        self.len_in(&SpaceId(crate::FIXTURE_SPACE.into()))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The last group appended to a space's chain, for assertions about what
    /// a write actually recorded — its actor, its commands, and whether the
    /// door flagged it as rule-6 owner administration.
    pub fn last_in(&self, space: &SpaceId) -> Option<CommandGroup> {
        let n = self.store.len(space);
        self.store.get(space, n.checked_sub(1)?).map(|e| e.group)
    }

    /// The last group in the conformance space's chain.
    pub fn last(&self) -> Option<CommandGroup> {
        self.last_in(&SpaceId(crate::FIXTURE_SPACE.into()))
    }
}

impl Default for WriteLog {
    fn default() -> Self {
        WriteLog::new()
    }
}

/// A stated belief about the addressed row's version: `None` = the braces
/// said nothing, `Some(None)` = `rev: null` (create-once), `Some(Some(n))`
/// = "the row is at version n".
type RevBelief = Option<Option<i64>>;

/// **Identity precedence** (§6 rule 8) now lives in `ordo-plan`, because
/// the mode label is a *compiler artifact*: `ordo plan`'s artifact JSON
/// and `ordo check`'s report both print it, and neither can depend on the
/// conformance harness to learn it. Re-exported here so the write protocol
/// (and `ordo-host`, which reads the label off this path) keeps its one
/// import and there stays exactly one classifier.
pub use ordo_plan::writes::{set_mode, SetMode};

/// One evaluated field value: what the create gate reasons over, plus its
/// SQL literal spelling.
#[derive(Debug, Clone)]
struct EvaledVal {
    gate: CreateVal,
    sql: String,
}

/// **The three shapes a write's right-hand side can have** — 17f, DL §6
/// ("RHS field references") and DL §3 ("Legal anywhere expressions are: …
/// write RHS (per-row values in bulk writes and rules)").
///
/// The distinction that matters is not "literal or expression"; it is
/// **when the value exists**. The write protocol does a great deal with a
/// value before the row lands — proves a create's guard against the
/// would-be row, seals it, digests it for a blind index, addresses a unique
/// key with it, checks it against a closed ladder, images it — and all of
/// that needs the value in hand. So a value that can be known early is made
/// known early ([`Rhs::RowFree`], folded), and a value that cannot be is
/// admitted only where nothing needs it early ([`Rhs::RowReading`]).
#[derive(Debug, Clone)]
enum Rhs {
    /// The closed set the platform already held: literals, bound
    /// parameters, `$caller`. Evaluated by [`eval_expr`], unchanged.
    Held,
    /// An expression with no field reference. It has an answer before the
    /// statement runs, and [`Fixture::fold_computed_rhs`] gets that answer
    /// from the database and rewrites the statement to carry it — so
    /// nothing downstream ever sees this variant.
    RowFree(String),
    /// An expression that reads the row being written. Rendered into the
    /// statement's own `SET`, which is what makes `used + 1` atomic.
    RowReading(String),
}

impl Fixture {
    /// Execute a write op as a caller: `run_write_as(pg, log,
    /// `"renameTask($task: \"task_1\", $title: \"x\")"`, Some("mem_1"))`.
    ///
    /// Success returns a summary (`[{"created": id}]`, `[{"updated": n}]`…)
    /// and appends one command group to the log. Failure returns the error
    /// class first, like every other harness entry point.
    pub fn run_write_as(
        &self,
        pg: &Pg,
        log: &mut WriteLog,
        call: &str,
        caller: Option<&str>,
    ) -> Result<Value, String> {
        // NOT atomic, and deliberately not pretending to be: each `psql`
        // call is its own process, so a `BEGIN` here would end the instant
        // that process exited — a transaction in name only, and a `FOR
        // UPDATE` lock released before the next statement ever ran.
        //
        // Atomicity lives on the executor path (`run_write_on` against an
        // open transaction from `ordo_exec::Pool::begin`), which is where
        // AA-004 was actually closed and where the conformance suite
        // proves it. This entry point exists so the harness can drive the
        // protocol without a runtime; a multi-statement op that fails
        // partway through it leaves earlier statements applied.
        let out = self.run_write_on(pg, log, call, caller);
        // This path is not transactional (see above), so the effect is
        // already applied by the time we get here and the group belongs in
        // the chain immediately.
        match &out {
            Ok(_) => log.commit_pending()?,
            Err(_) => log.drop_pending(),
        }
        out
    }

    /// The protocol itself, against any [`WriteDb`]. When that db is an
    /// open transaction, preconditions, row locks, and mutations share one
    /// snapshot and commit or abort as a unit.
    /// The door step, then the protocol.
    ///
    /// DL §3 bootstrap **rule 6 [settled]**: *"A space owner may grant,
    /// revoke, and change levels on every acl-derived link entity in the
    /// space **without holding an access row on anything**… the owner
    /// capability is evaluated **at the door, once per call, before
    /// planning** — not as an extra disjunct in each row's guard. When it
    /// applies, the derived link entity's guards are not consulted for that
    /// call."*
    ///
    /// None of that existed. On the shipped fixture the space owner got
    /// `not_found` on grant and revoke, and `setBoardsAccessLevel` answered
    /// **`updated: 0` inside a success envelope** — a silent no-op on an
    /// authorization change. That is the permanent lockout the rule exists
    /// to break, and it is the repair path 10b's closure rests on: 10b ruled
    /// a guest- or routine-created ungranted row safe *because* the owner
    /// could always administer it.
    ///
    /// The mechanism is the spec's, not a convenient one. The alternative —
    /// an extra `or caller is owner` disjunct inside each link entity's
    /// guard — is what the rule explicitly forbids, and for a good reason:
    /// it makes ownership a row-level fact that every guard must re-derive,
    /// which is the per-row reading that reintroduces the deadlock.
    pub fn run_write_on(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        call: &str,
        caller: Option<&str>,
    ) -> Result<Value, String> {
        match self.owner_admin_view(db, call, caller)? {
            // Guards not consulted, by not being there: the call runs
            // against a schema view whose acl link entities carry none.
            // Nothing downstream needs a flag to DECIDE anything, so nothing
            // downstream can forget to check one.
            //
            // The `true` here is the other half of rule 6 — *"logged with
            // the owner as actor and flagged as owner administration"* — and
            // it is not a second decision. **This match arm is the whole
            // producer of the flag**: the only place in the tree that hands
            // `run_write_body` anything but `false`, and it is the same
            // expression that granted the exemption. There is no path on
            // which a group is flagged without the door having decided, and
            // none on which the door decides and the group is not flagged.
            Some(view) => view.run_write_body(db, log, call, caller, true),
            None => self.run_write_body(db, log, call, caller, false),
        }
    }

    /// Does rule 6 apply to this call? Answered once, before planning.
    ///
    /// Both halves must hold: the caller is a space **owner**, and every
    /// statement targets an **acl-derived link entity**. The second half is
    /// what keeps this a scoped capability rather than a bypass — an owner
    /// writing `tasks` is an ordinary caller and meets the ordinary guard.
    fn owner_admin_view(
        &self,
        db: &dyn WriteDb,
        call: &str,
        caller: Option<&str>,
    ) -> Result<Option<Fixture>, String> {
        let Some(caller) = caller else {
            return Ok(None);
        };
        let Ok(pl) = parse_pipeline(call) else {
            return Ok(None);
        };
        let [src] = pl.sources.as_slice() else {
            return Ok(None);
        };
        let Some(op) = self.registry.get(&src.name.name) else {
            return Ok(None);
        };
        let OpBody::Write(stmts) = &op.body else {
            return Ok(None);
        };
        // Every statement, not any: a multi-statement op that touches one
        // link entity and one ordinary one is not owner administration, and
        // clearing guards for it would exempt the ordinary write too.
        if stmts.is_empty()
            || !stmts.iter().all(|st| {
                stmt_entity(st)
                    .is_some_and(|e| ordo_schema::acl_link_root(&self.schema, e).is_some())
            })
        {
            return Ok(None);
        }
        if !self.is_space_owner(db, caller)? {
            return Ok(None);
        }
        Ok(Some(self.with_acl_guards_cleared()))
    }

    /// The caller's role, read from `members` — the same column the door's
    /// access engine reads.
    fn is_space_owner(&self, db: &dyn WriteDb, caller: &str) -> Result<bool, String> {
        let rows = db.rows(&format!(
            "SELECT \"role\" FROM \"members\" WHERE {sp} AND \"id\" = {}",
            sql_str(caller),
            sp = self.space_pred(),
        ))?;
        Ok(rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .as_deref()
            == Some("owner"))
    }

    /// This fixture with every acl link entity's guards removed — **and its
    /// root's**.
    ///
    /// The root is not an oversight. Rule 6 says the owner administers acl
    /// rows *"without holding an access row **on anything**"*, and creating
    /// one is a write whose `board` ref is checked against the ROOT's write
    /// guard (an owned ref requires write on its target). Clearing only the
    /// link left `grant` answering `not_found` on the board — the owner holds
    /// no access row there, which is the precise condition the rule waives.
    /// Revoke and the level change passed, because neither validates a ref;
    /// the asymmetry is what made the cause legible.
    ///
    /// It is not a widening: `owner_admin_view` has already established that
    /// **every statement in this call targets a link entity**, so no
    /// statement can write the root. Clearing its guards affects one thing —
    /// whether the ref check can see the row being pointed at.
    fn with_acl_guards_cleared(&self) -> Fixture {
        let mut cleared: Vec<String> = Vec::new();
        for e in &self.schema.entities {
            if let Some(root) = ordo_schema::acl_link_root(&self.schema, &e.name) {
                cleared.push(e.name.clone());
                cleared.push(root);
            }
        }
        let mut schema = self.schema.clone();
        for e in &mut schema.entities {
            if cleared.contains(&e.name) {
                e.guards = None;
            }
        }
        self.with_schema(schema)
    }

    /// `owner_administration` is the door's rule-6 answer, carried down to
    /// the one place that builds the [`CommandGroup`] — a parameter rather
    /// than a field on the fixture or a flag set on the group afterwards,
    /// so the group is *constructed* flagged and there is no window in which
    /// a rule-6 group exists unflagged. It is a log input and nothing else;
    /// nothing below reads it to decide anything.
    fn run_write_body(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        call: &str,
        caller: Option<&str>,
        owner_administration: bool,
    ) -> Result<Value, String> {
        // Wrapped once, here, so every statement the protocol issues below
        // is audited on its way out (see `SpaceAudit`). Nothing further
        // down holds the unwrapped db.
        let audited = SpaceAudit {
            inner: db,
            space: self.space().0.clone(),
        };
        let db: &dyn WriteDb = &audited;
        // Sealed payloads staged by a previous write that never reached
        // its flush (a refusal, a rolled-back transaction) are not this
        // write's to store. Cleared here rather than on every error path,
        // because there are a great many error paths.
        log.payloads.clear();
        // The call form is a source line; the pipeline parser already
        // understands it.
        let pl = parse_pipeline(call).map_err(|e| format!("parse: {} at {}", e.message, e.span))?;
        let [src] = pl.sources.as_slice() else {
            return Err("a write call is one op, not a source list".into());
        };
        let op = self
            .registry
            .get(&src.name.name)
            .ok_or_else(|| format!("unknown_source: no op `{}`", src.name.name))?
            .clone();
        let OpBody::Write(stmts) = &op.body else {
            return Err(format!(
                "`{}` is a read op — run it, don't write it",
                op.name.name
            ));
        };
        // G15's write half: anonymous callers have no write view at all.
        // *Which* refusal they get is R-G10's business, not this line's —
        // see `anonymous_refusal`.
        let Some(caller) = caller else {
            return Err(self.anonymous_refusal(stmts));
        };

        // ── bind the op's parameters ────────────────────────────────────
        //
        // `fold_neg_lit` (`ordo-sema`, shared with the read path's
        // `param_env_with`/`const_expr` now that both fold the same way)
        // still has to run HERE, before `params` is built, rather than only
        // inside `ordo_sema::write_value_expr`'s own environment loop: this
        // vector is also read directly by `is_held_value` and
        // `classify_rhs` below, which pattern-match a parameter's bound
        // Expr by shape (`IntLit`, `DecimalLit`, …) to decide whether a
        // right-hand side is a value the protocol can hold or one it must
        // compute. An un-folded `Neg { IntLit(2500) }` matches neither arm,
        // so a negative literal would silently misclassify as computed
        // rather than fail to bind — a different, quieter bug than the one
        // this fixes. Folding is idempotent, so this and
        // `write_value_expr`'s own fold never disagree.
        let mut params: Vec<(String, Expr)> = Vec::new();
        for a in &src.args {
            let SourceArg::Named { name, value, .. } = a else {
                return Err("unbound_required_param: write calls bind by name".into());
            };
            params.push((name.name.clone(), fold_neg_lit(value)));
        }
        for p in &op.params {
            if params.iter().any(|(n, _)| n == &p.name.name) {
                continue;
            }
            match &p.default {
                Some(d) => params.push((p.name.name.clone(), d.clone())),
                None => {
                    return Err(format!(
                        "unbound_required_param: `${}` has no value and no default",
                        p.name.name
                    ))
                }
            }
        }

        // ── plan every statement's mutations, then run them as one batch ─
        let mut summaries = Vec::new();
        let mut mutation_sql: Vec<String> = Vec::new();
        let mut commands: Vec<PendingCommand> = Vec::new();
        let mut post: Vec<PostRow> = Vec::new();
        // `let x = set E { … }` — the row descriptors bound by the
        // statements planned so far (DL §3, "Naming a freshly created row
        // in the same op"). Statements are planned in order, so a name
        // reaches only the statements after the one that bound it, which
        // is exactly the scope rule; the registration gate
        // (`ordo_plan::writes`) has already proved that every reference
        // here resolves and that every bound statement is a create.
        let mut bound: Vec<(String, Vec<(String, Expr)>)> = Vec::new();
        // `(entity, id)` for every row this group has already created —
        // see `plan_insert`'s `group` parameter for why the parent check
        // needs it and why admitting them is not a hole in G12.
        let mut group_rows: Vec<(String, String)> = Vec::new();
        for stmt in stmts {
            // Substituted BEFORE planning, so the whole write protocol —
            // guards, refs, images, the command log — sees a statement
            // whose right-hand sides are the ordinary closed set. A
            // descriptor member is not a new kind of value; it is the
            // expression the create already wrote, plus the id the mint
            // just produced.
            let stmt = &substitute_bound(stmt, &bound)?;
            // Then the row-free half of every computed right-hand side, for
            // the same reason and by the same means: after this, a value the
            // platform can hold IS one, and only the values that genuinely
            // need the row are still expressions.
            // Money literals in major units become the minor units the
            // column holds, once, here (27ak). After this a money field's
            // literal is in stored units and nothing converts it again.
            let stmt = &self.fit_scaled_literals(stmt, &params)?;
            let stmt = &self.fold_computed_rhs(db, stmt, &params, caller)?;
            let s = self.plan_write_stmt(db, log, stmt, &params, caller, &group_rows)?;
            if let (Some(entity), Some(id)) = (
                s.summary.get("entity").and_then(|v| v.as_str()),
                s.summary.get("created").and_then(|v| v.as_str()),
            ) {
                group_rows.push((entity.to_string(), id.to_string()));
            }
            if let WriteStmt::Set {
                binding: Some(name),
                fields,
                ..
            } = stmt
            {
                let Some(id) = s.summary.get("created").and_then(|v| v.as_str()) else {
                    // Unreachable through a registered op: `unbindable_write`
                    // refuses every non-creating binding at registration.
                    // Kept because "the descriptor is empty" would otherwise
                    // surface as a null id in somebody's foreign key.
                    return Err(format!(
                        "internal: `let {}` bound a statement that created no row",
                        name.name
                    ));
                };
                bound.push((name.name.clone(), descriptor(id, fields)));
            }
            summaries.push(s.summary);
            mutation_sql.extend(s.sql);
            commands.extend(s.commands);
            post.extend(s.post);
        }
        for stmt in &mutation_sql {
            db.rows(stmt)?;
        }
        // The post-image obligation: the write guard must still hold for
        // every row as it now IS. A caller who moved a row into a parent
        // they do not control fails here, and (on the transactional path)
        // the whole group rolls back — so the escalation never lands.
        for row in &post {
            let (entity, id) = match row {
                PostRow::Id { entity, id } => (entity.clone(), id.clone()),
                PostRow::Key {
                    entity,
                    columns,
                    literals,
                    expect,
                } => {
                    let found = self.row_by_key(db, entity, columns, literals, false)?;
                    match (found, expect) {
                        (None, _) => {
                            return Err(format!(
                                "internal: the keyed write on `{entity}` left no row holding \
                                 its own addressing key"
                            ))
                        }
                        // The create branch's race, answered as the
                        // caller's belief demands: a create-once statement
                        // that finds another row holding its key did not
                        // create, and must not report that it did.
                        (Some(other), Some(want)) if &other != want => {
                            return Err(format!(
                                "expected_absent: `{entity}` row `{other}` already holds this \
                                 write's key — it was created while this create-once statement \
                                 ran, and nothing here was applied to it"
                            ))
                        }
                        (Some(found), _) => (entity.clone(), found),
                    }
                }
            };
            if !self.row_visible(db, &entity, &id, caller, true)? {
                return Err(format!(
                    "access_guard: the change would move `{id}` ({entity}) outside your write view"
                ));
            }
        }

        // ── after-images for converges, then the group append ───────────
        let mut group_cmds = Vec::new();
        for c in commands {
            let after = match c.kind {
                CommandKind::Create => c.after,
                CommandKind::Delete => None,
                CommandKind::Converge => Some(
                    self.row_image(db, log, &c.entity_name, &c.row_id)?
                        .ok_or_else(|| {
                            format!("converged row `{}` vanished mid-batch", c.row_id)
                        })?,
                ),
            };
            group_cmds.push(Command {
                id: CommandId(format!("cmd_{}", log.mint())),
                space: SpaceId(self.space().0.clone()),
                entity: c.entity_name,
                row_id: c.row_id,
                kind: c.kind,
                before: c.before,
                after,
                at: Timestamp(log.stamp()),
            });
        }
        if !group_cmds.is_empty() {
            let group = CommandGroup {
                // The batch's correlation when a bulk import has one open,
                // a fresh one otherwise. See `WriteLog::group_correlation`.
                correlation: log.group_correlation(),
                actor: log.actor_for(caller),
                commands: group_cmds,
                // Rule 6: *"logged with the owner as actor and flagged as
                // owner administration"*. The actor above is the first half;
                // this is the second. Decided at the door — see
                // `run_write_on` — and merely recorded here.
                owner_administration,
                // An ordinary write, never a revert, so there is nothing to
                // have restored, verified or not. `ordo_exec::undo::apply`
                // is the one producer of `true`.
                restored_unverified: false,
                restore: None,
            };
            // The payloads the images name, stored now that the group they
            // belong to has an id. This rides the caller's transaction, so
            // on the executor path a payload and the row it describes
            // commit together or not at all — and the handles in the group
            // about to be appended are resolvable the instant the group is.
            for p in std::mem::take(&mut log.payloads) {
                db.rows(&ordo_crypto::sealed::insert_sql(
                    self.space(),
                    &p.handle,
                    p.epoch,
                    &p.entity,
                    &p.field,
                    &p.anchor,
                    &group.correlation.0,
                    &p.bytes,
                ))?;
            }
            // NOT appended here. The chain is the audit spine, and a group
            // in it asserts that a write happened — so it must not be
            // written until the write is committed. The caller appends
            // after commit; a transaction that aborts (a fence, a failed
            // commit) therefore leaves no phantom link behind.
            //
            // This is as close to "the append rides the transaction" as an
            // in-memory store can get. A durable Postgres store would put
            // the append INSIDE the transaction and get the property for
            // real; until then the ordering is the guarantee, and it is
            // stated rather than assumed.
            //
            // Inside a batch this EXTENDS the pending group rather than
            // replacing it, which is how a bulk import turns N calls into
            // one group and one chain entry.
            log.stage_group(group)?;
        }
        Ok(Value::Array(summaries))
    }

    /// The platform's own converge: one blob upload, run as the write the
    /// protocol runs for everything else.
    ///
    /// Before this existed, `ordo_exec::blob::attach` inserted the
    /// `$blobref` row and `UPDATE`d the entity column itself, in the blob
    /// door's own transaction, and appended no command group. An upload was
    /// therefore unrevertible, fired no `converged` and no `written`, and
    /// did not move `rev`, so a caller holding a version from before an
    /// upload never learned it was stale. And it was the only entity-row
    /// `UPDATE` in the tree that appended no group at all: the membership
    /// surface (`membership.rs`) writes `members` and `groupMembers`
    /// outside this file too, but it images before and after and stages a
    /// command group for every change it makes. What `attach` did had no
    /// record anywhere, which is what made "the write protocol is the only
    /// writer" a rule to remember rather than a rule a test can enforce.
    ///
    /// **There is no call string and no parsed op here**, which is the
    /// other half of why the seam is safe. A `PlatformValue` has no textual
    /// form and no parser production, `Sema::check_content_params` refuses
    /// a blob-typed op parameter, and this entry point is reached from
    /// `ordo_exec::Executor::attach_write` alone. So no `.ordo` source can
    /// name a platform value and no op body can reach this function, which
    /// is what `no_op_body_can_reach_the_platform_converge` asserts.
    ///
    /// The value is consumed **by value**: a second converge needs a second
    /// upload, because a token is minted from a `Stored` and a `Stored` is
    /// what `BlobPipeline::put` returns for bytes it actually wrote.
    pub fn run_platform_converge(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        value: impl PlatformValue,
        caller: &str,
    ) -> Result<Value, String> {
        // Wrapped here for `run_write_body`'s reason, and it matters more
        // on this path: one of the two statements below is built in
        // `ordo-crypto`, which cannot see this audit. It passes because
        // `$blobref` spells its tenancy column `"$space"`, which is the
        // reason `blobref.rs` gives for spelling it that way, so the
        // reference insert is confined by the same mechanism that confines
        // the `UPDATE` beside it.
        let audited = SpaceAudit {
            inner: db,
            space: self.space().0.clone(),
        };
        let db: &dyn WriteDb = &audited;
        log.payloads.clear();

        // Entity, field and row id come out of the value and from nowhere
        // else. There is no parallel argument to disagree with the token,
        // so there is no call shape in which an upload lands on a row it
        // was not minted for.
        let (entity, field, id) = value.target();
        let e = self.entity_for_write(entity)?;
        // The tier was chosen at the door, from the schema. What is checked
        // here is narrower and is this path's own business: a platform
        // value may only set a content column. Setting `title` from a token
        // would be a write with no expression behind it on a field an op
        // could have written, which is the one thing this seam must not
        // become.
        if !e.field(field).is_some_and(|f| f.is_content()) {
            return Err(format!(
                "internal: `{entity}.{field}` is not a content field, and a platform value \
                 writes nothing else"
            ));
        }

        // The same four steps an addressed set takes, in the same order and
        // for the same reasons: lock first so the guard decision and the
        // mutation are one decision, read guard before write guard so a row
        // outside the read view is indistinguishable from an absent one,
        // then the before-image under that lock.
        self.lock_row(db, entity, id)?;
        if !self.row_visible(db, entity, id, caller, false)? {
            return Err(not_found());
        }
        if !self.row_visible(db, entity, id, caller, true)? {
            return Err(format!("access_guard: write `{id}` as {caller}"));
        }
        let before = self.row_image(db, log, entity, id)?.ok_or_else(not_found)?;

        // **The reference, then the column, in one batch.** A `$blobref`
        // row with no column pointing at it is a leaked object the sweep
        // collects, which costs money. A column with no `$blobref` row
        // names nothing, has nowhere to get its mime and size from, and the
        // shape gate cannot see it because the column's type is still
        // right. `ordo-exec`'s blob module argues that asymmetry at length;
        // this is where it is now spelled, and reversing these two lines is
        // how it would be lost.
        let mutations = [
            value.reference_sql(),
            format!(
                "UPDATE {} SET {} = {}, {rev} = {rev} + 1 WHERE {sp} AND {} = {};",
                q_ident(entity),
                q_ident(field),
                value.column_literal(),
                q_ident("id"),
                sql_str(id),
                sp = self.space_pred(),
                rev = q_ident("rev"),
            ),
        ];
        for stmt in &mutations {
            db.rows(stmt)?;
        }

        // The post-image obligation, exactly as `run_write_body` states it:
        // the write guard must hold for the row as it now IS. On the
        // executor path a failure here rolls the whole transaction back, so
        // an upload that moved a row out of the caller's write view leaves
        // no column and no reference behind.
        if !self.row_visible(db, entity, id, caller, true)? {
            return Err(format!(
                "access_guard: the change would move `{id}` ({entity}) outside your write view"
            ));
        }

        let after = self
            .row_image(db, log, entity, id)?
            .ok_or_else(|| format!("converged row `{id}` vanished mid-batch"))?;
        // The image is read back from the row, never taken from the token,
        // so this compares two independently derived answers to "what does
        // the column hold now". A disagreement means the statement above
        // set something other than the object that was stored, and a group
        // asserting an upload that did not happen is worse than a refusal.
        if after.get(field) != Some(&value.image_value()) {
            return Err(format!(
                "internal: `{entity}.{field}` does not hold the object this upload stored"
            ));
        }

        let command = Command {
            id: CommandId(format!("cmd_{}", log.mint())),
            space: SpaceId(self.space().0.clone()),
            entity: entity.to_string(),
            row_id: id.to_string(),
            kind: CommandKind::Converge,
            before: Some(before),
            after: Some(after),
            at: Timestamp(log.stamp()),
        };
        let group = CommandGroup {
            correlation: CorrelationId(format!("cg_{}", log.mint())),
            actor: log.actor_for(caller),
            commands: vec![command],
            // Rule 6 is about acl-derived link entities and an owner
            // administering them. A blob column is never one, so this is
            // `false` by the same argument that makes `run_write_on`'s
            // match arm the flag's only producer.
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        };
        for p in std::mem::take(&mut log.payloads) {
            db.rows(&ordo_crypto::sealed::insert_sql(
                self.space(),
                &p.handle,
                p.epoch,
                &p.entity,
                &p.field,
                &p.anchor,
                &group.correlation.0,
                &p.bytes,
            ))?;
        }
        // Held back, appended on commit, for `run_write_body`'s reason: a
        // group in the chain asserts that a write happened.
        log.pending = Some(group);
        Ok(json!([{ "entity": entity, "updated": 1 }]))
    }

    /// **A member leaving a tenant, written** (checklist 27aq,
    /// `plan-member-release.md` §3): the app's `remove` and `reassign`
    /// lines, the platform's own effects, and the seat, as **one command
    /// group** in the caller's transaction.
    ///
    /// The door has already decided the leave is allowed: it locked the
    /// seat, evaluated the platform's conditions and the program's release
    /// rule, and resolved each `reassign` to the member it names. What
    /// arrives here is that decision as data ([`ReleaseStep`]), and this is
    /// the write. It takes the protocol's own path for every row it touches:
    /// a row lock, a before-image, the statement, an after-image for a
    /// converge, and `rev` moved by the statement itself. A delete walks
    /// owned children and refuses plain referrers exactly as an op's `del`
    /// does. So the group is revertible, joins the space's chain in the
    /// transaction that commits it, and fires what any write fires.
    ///
    /// # How guards apply
    ///
    /// **Entity guards are not re-evaluated against the leaver**, neither
    /// before nor after. The program's release rule is the app's authority
    /// for these effects: an author who writes `remove drafts.author` has
    /// said that a leaver's drafts go, and a leaver who could not delete one
    /// of them through an ordinary op is exactly the case the line exists
    /// for. Re-checking the leaver's write view would turn the app's rule
    /// into a guess about who happens to hold access, and would leave rows
    /// pointing at a seat nobody can clear. This is the same shape as rule
    /// 6's owner administration, a capability decided once at the door, and
    /// like it the decision is not hidden: every row is imaged, the group is
    /// attributed to the leaver, and the door records the act. What is
    /// still enforced is structure, not access: a tree node with children is
    /// not deleted, a row other rows refer to plainly is not deleted
    /// (`has_referrers`), and a unique or foreign key the database holds
    /// still holds.
    ///
    /// # Order
    ///
    /// The steps in the order given (the program's lines, then the
    /// platform's acl rows), each seeing the ones before it; then the seat's
    /// group memberships, through the membership surface's own removal so
    /// `memberCount` moves with each link; then the seat itself
    /// ([`Fixture::release_seat`]). One batch correlation joins them, so the
    /// chain gets one entry and a revert of it undoes the whole leave.
    ///
    /// Returns one summary per entity touched, for the executor's
    /// invalidation. `not_found` when `seat` is not a principal seat here.
    pub fn run_release_on(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        seat: &str,
        steps: &[ReleaseStep],
        left_name: Option<&str>,
    ) -> Result<Value, String> {
        let audited = SpaceAudit {
            inner: db,
            space: self.space().0.clone(),
        };
        let db: &dyn WriteDb = &audited;
        log.payloads.clear();
        // One group for the whole leave. Opened here and closed on every
        // path out, so a refusal does not leave the attempt's log batching.
        let opened = !log.in_batch();
        if opened {
            let correlation = format!("cg_{}", log.mint());
            log.open_batch(correlation);
        }
        let out = self.release_body(db, log, seat, steps, left_name);
        if opened {
            log.close_batch();
        }
        out
    }

    /// **The seat of a person who deleted their account** (owner ruling
    /// 2026-10-06): a `left` seat becomes `deleted`, as one converge in one
    /// command group of the tenant's chain, attributed to the seat. The
    /// name and every row that points at the seat stay. `Value::Null` when
    /// the seat is not a `left` principal seat here, which is what makes a
    /// second pass over the same seat a no-op.
    pub fn run_seat_deleted_on(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        seat: &str,
    ) -> Result<Value, String> {
        let audited = SpaceAudit {
            inner: db,
            space: self.space().0.clone(),
        };
        let db: &dyn WriteDb = &audited;
        log.payloads.clear();
        if !self.mark_seat_deleted(db, log, seat)? {
            return Ok(Value::Null);
        }
        Ok(json!([{ "entity": "members", "updated": 1, "deleted": seat }]))
    }

    fn release_body(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        seat: &str,
        steps: &[ReleaseStep],
        left_name: Option<&str>,
    ) -> Result<Value, String> {
        let mut summaries = Vec::new();
        let mut commands: Vec<PendingCommand> = Vec::new();
        for step in steps {
            let (entity, field) = match step {
                ReleaseStep::Remove { entity, field } => (entity, field),
                ReleaseStep::Reassign { entity, field, .. } => (entity, field),
            };
            let e = self.entity_for_write(entity)?;
            if e.field(field).is_none() {
                return Err(format!("internal: `{entity}` has no field `{field}`"));
            }
            let ids = first_col(db.rows(&format!(
                "SELECT {} FROM {} WHERE {sp} AND {} = {} ORDER BY {} FOR UPDATE",
                q_ident("id"),
                q_ident(entity),
                q_ident(field),
                sql_str(seat),
                q_ident("id"),
                sp = self.space_pred(),
            ))?);
            match step {
                ReleaseStep::Remove { .. } => {
                    let n = ids.len();
                    if n > 0 {
                        for id in &ids {
                            self.refuse_tree_parent(db, &e, id)?;
                        }
                        let roots: Vec<(String, String)> =
                            ids.iter().map(|id| (entity.clone(), id.clone())).collect();
                        let mut doomed = self.owned_cascade(db, &roots)?;
                        self.lock_cascaded(db, &doomed, &roots)?;
                        self.refuse_plain_referrers(db, &doomed)?;
                        if doomed.len() > 1 {
                            doomed = self.delete_order(db, doomed)?;
                        }
                        let (cmds, sql) = self.cascade_delete_plan(db, log, &doomed)?;
                        for stmt in &sql {
                            db.rows(stmt)?;
                        }
                        commands.extend(cmds);
                    }
                    summaries.push(json!({
                        "entity": entity, "field": field, "deleted": n,
                    }));
                }
                ReleaseStep::Reassign { to, .. } => {
                    for id in &ids {
                        let before = self
                            .row_image(db, log, entity, id)?
                            .ok_or_else(|| format!("selected row `{id}` vanished"))?;
                        db.rows(&format!(
                            "UPDATE {} SET {} = {}, {rev} = {rev} + 1 WHERE {sp} AND {} = {};",
                            q_ident(entity),
                            q_ident(field),
                            sql_str(to),
                            q_ident("id"),
                            sql_str(id),
                            sp = self.space_pred(),
                            rev = q_ident("rev"),
                        ))?;
                        let after = self
                            .row_image(db, log, entity, id)?
                            .ok_or_else(|| format!("converged row `{id}` vanished mid-batch"))?;
                        commands.push(PendingCommand {
                            entity_name: entity.clone(),
                            row_id: id.clone(),
                            kind: CommandKind::Converge,
                            before: Some(before),
                            after: Some(after),
                        });
                    }
                    summaries.push(json!({
                        "entity": entity, "field": field, "updated": ids.len(), "to": to,
                    }));
                }
            }
        }
        if !commands.is_empty() {
            let group_cmds = commands
                .into_iter()
                .map(|c| Command {
                    id: CommandId(format!("cmd_{}", log.mint())),
                    space: SpaceId(self.space().0.clone()),
                    entity: c.entity_name,
                    row_id: c.row_id,
                    kind: c.kind,
                    before: c.before,
                    after: c.after,
                    at: Timestamp(log.stamp()),
                })
                .collect();
            let correlation = log.group_correlation();
            log.stage_group(CommandGroup {
                correlation,
                actor: Actor::User(seat.to_string()),
                commands: group_cmds,
                // The release rule is not rule 6, whoever leaves.
                owner_administration: false,
                restored_unverified: false,
                restore: None,
            })?;
        }

        // The seat's group memberships, one link at a time through the
        // membership surface, so each link's removal and its group's
        // recount are imaged together. Before the seat changes: the surface
        // admits an actor who holds a role here, and the leaver still does.
        let groups = first_col(db.rows(&format!(
            "SELECT {} FROM {} WHERE {sp} AND {} = {} ORDER BY {}",
            q_ident("group"),
            q_ident("groupMembers"),
            q_ident("member"),
            sql_str(seat),
            q_ident("group"),
            sp = self.space_pred(),
        ))?);
        for group in &groups {
            self.remove_from_group(db, log, seat, group, seat)?;
        }
        if !groups.is_empty() {
            summaries.push(json!({ "entity": "groupMembers", "deleted": groups.len() }));
        }

        if !self.release_seat(db, log, seat, left_name)? {
            return Err(NOT_FOUND.to_string());
        }
        summaries.push(json!({ "entity": "members", "updated": 1, "released": seat }));

        // The payloads the images name, under the one correlation the whole
        // leave carries, for `run_write_body`'s reason.
        let correlation = log
            .pending()
            .map(|g| g.correlation.0.clone())
            .ok_or_else(|| "internal: a leave staged no group".to_string())?;
        for p in std::mem::take(&mut log.payloads) {
            db.rows(&ordo_crypto::sealed::insert_sql(
                self.space(),
                &p.handle,
                p.epoch,
                &p.entity,
                &p.field,
                &p.anchor,
                &correlation,
                &p.bytes,
            ))?;
        }
        Ok(Value::Array(summaries))
    }

    /// **R-G12 routed through R-G10**: what an anonymous caller's write is
    /// told, and why it is decided without a query.
    ///
    /// *That* an anonymous write is refused is not a guard decision at all.
    /// A committed write appends a `CommandGroup`, and a group carries an
    /// `Actor`; there is no anonymous `Actor` and there must not be one,
    /// because an unattributable entry in the audit spine is worse than a
    /// refusal. So the refusal stands for every shape of write.
    ///
    /// The **class** is a guard decision, and it is the one this used to get
    /// wrong. R-G12 makes every guard evaluate false for a caller with no
    /// principal, so on a **guarded** entity every row is outside their read
    /// view — and R-G10 then requires an addressed write to answer
    /// `not_found`, indistinguishable from a row that never existed.
    /// Answering `access_guard` there let an anonymous caller holding a
    /// stolen id list sort the real ids from the invented ones, which is the
    /// enumeration oracle R-G10 exists to close.
    ///
    /// Everything else answers `access_guard`, in R-G12's own words because
    /// "there is no addressed row to be coy about, and the guard simply
    /// evaluates false": a **create** names no existing row; a **bulk**
    /// write names a range rather than a row; an **unguarded** entity has no
    /// read view for a row to be outside of (R-G10's stated exception, and
    /// the reason `not_found` would be a lie there — it would claim absence
    /// about a row set that is not a confidentiality boundary).
    ///
    /// **Why no query.** Deciding this from the schema alone is not a
    /// shortcut past the guard; it is the guard's answer, computed. R-G12
    /// makes the guard false for *every* row by rule, so no row's existence
    /// can change the outcome — and that is the strongest form of the
    /// non-oracle property available, because there is no query whose result
    /// could differ between an id that exists and one that does not. The
    /// database is never consulted, so it cannot leak by result, by row
    /// count, or by time.
    ///
    /// The **first** statement decides, because statements are planned in
    /// order and the first refusal aborts the whole group — the same
    /// statement that would have decided for a named caller.
    fn anonymous_refusal(&self, stmts: &[WriteStmt]) -> String {
        let Some(stmt) = stmts.first() else {
            return "access_guard: anonymous callers cannot write".into();
        };
        let (entity, addressed) = match stmt {
            WriteStmt::Set { entity, cond, .. } => {
                (&entity.name, cond.as_ref().is_some_and(addresses_id))
            }
            WriteStmt::Del { entity, cond, .. } => (&entity.name, addresses_id(cond)),
        };
        // "Guarded" is exactly what the read-side guard compiler says it is,
        // asked with no caller — the same call `row_visible` would make.
        let guarded = matches!(
            guard_filter_query(&self.schema, entity, false, None),
            Ok(Some(_))
        );
        if addressed && guarded {
            NOT_FOUND.to_string()
        } else {
            "access_guard: anonymous callers cannot write".into()
        }
    }

    fn plan_write_stmt(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        stmt: &WriteStmt,
        params: &[(String, Expr)],
        caller: &str,
        group: &[(String, String)],
    ) -> Result<PlannedStmt, String> {
        // Every verb passes through here, which is why the platform-entity
        // refusal lives at the dispatch rather than in each arm: a check the
        // next verb can forget is not a check.
        //
        // DL §3 makes a platform entity's fields "all read-only to domain
        // code" and says "there is no domain-authored path to minting one".
        // Nothing enforced it. `set members { id: $id, role: "owner" }`
        // compiled, registered and ran — and `deployment_engine` builds the
        // door's access engine by reading the very column a domain op could
        // write, so an editor could promote themselves, and everyone else,
        // to owner. Proven against a real server before this landed.
        //
        // The class is `reserved_derived_name`, whose documented meaning is
        // already "the statement writes something the platform owns"; the
        // inventory row named `id` and `rev` because they were the only two
        // such things at the time.
        if let Some(name) = stmt_entity(stmt) {
            if self.schema.entity(name).is_some_and(|e| e.platform) {
                return Err(format!(
                    "reserved_derived_name: `{name}` is provided by the platform and is \
                     read-only to domain code — its rows are minted and changed by the \
                     platform's own paths, never by an op"
                ));
            }
        }
        match stmt {
            WriteStmt::Set {
                entity,
                cond: None,
                fields,
                ..
            } => self.plan_unconditioned_set(db, log, &entity.name, fields, params, caller, group),
            WriteStmt::Set {
                entity,
                cond: Some(c),
                fields,
                ..
            } => match addressed_id(c, params, caller)? {
                Some(id) => {
                    self.plan_addressed_set(db, log, &entity.name, &id, fields, params, caller)
                }
                None => self.plan_bulk(db, log, &entity.name, c, Some(fields), params, caller),
            },
            // Both sides of this dispatch changed: 6f added the subtree
            // branches, 6b/6c made `addressed_id` fallible (a null key is
            // `null_key_address`, not a silent demotion to a ranged write).
            // Both are needed.
            WriteStmt::Del {
                entity,
                cond,
                subtree,
                ..
            } => match (addressed_id(cond, params, caller)?, *subtree) {
                (Some(id), false) => self.plan_addressed_del(db, log, &entity.name, &id, caller),
                (Some(id), true) => self.plan_subtree_del(db, log, &entity.name, &id, caller),
                // A delete whose condition IS a unique key is addressed by
                // that key — R-G17's "`revoke` is an addressed delete by
                // the same key". The distinction is not decorative: a
                // ranged delete over an empty write view is a vacuous
                // no-op (R-G9), so a revoke the caller may not perform
                // would report success having deleted nothing. Addressed,
                // it answers `access_guard` (the row is readable but not
                // writable) or `not_found` (it is neither, or absent) —
                // which is what GC044 asserts and what an admin UI needs
                // to hear.
                (None, false) => {
                    match self.addressed_by_key(db, &entity.name, cond, params, caller)? {
                        Some(id) => self.plan_addressed_del(db, log, &entity.name, &id, caller),
                        None => self.plan_bulk(db, log, &entity.name, cond, None, params, caller),
                    }
                }
                // The grammar only admits `subtree` after the addressed
                // brace form, so the anchor is an `id ==` by construction;
                // the only way here is an anchor expression that did not
                // evaluate to an id at all.
                (None, true) => Err(format!(
                    "invalid_binding_expr: `del {} … subtree` needs an anchor id",
                    entity.name
                )),
            },
        }
    }

    // ── create / converge-or-create ─────────────────────────────────────

    /// A `set` with no condition. **Identity precedence (§6 rule 8)** picks
    /// the mode from the braces, and this is the only place that decision
    /// is made:
    ///
    /// - `id` present → id-addressing, always. Every other unique field in
    ///   the braces is an ordinary value. A statement that could mean two
    ///   things means the one the reader expects: the id.
    /// - exactly one covered unique key → converge-or-create on that key.
    /// - two or more → refused. Both keys are addresses, they can name
    ///   different rows, and no rule the platform could invent to pick
    ///   between them would be one the author could predict.
    #[allow(clippy::too_many_arguments)]
    fn plan_unconditioned_set(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        entity: &str,
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
        group: &[(String, String)],
    ) -> Result<PlannedStmt, String> {
        match set_mode(&self.schema, entity, fields) {
            SetMode::Ambiguous(keys) => Err(format!(
                "ambiguous_identity: `{entity}` braces cover {} unique keys ({}) — each of \
                 them addresses a row, and they can name different rows; address by one key \
                 and pass the others (if you mean them at all) on an id-addressed converge",
                keys.len(),
                keys.join(", ")
            )),
            SetMode::Addressed => {
                // Rule 8's first clause. The id addresses; the rest of the
                // braces are values, `rev` included (`split_rev` reads it
                // inside the addressed path, where `rev: null` is refused
                // as the incoherence it is).
                let (_, id_expr) = fields
                    .iter()
                    .find(|(n, _)| n.name == "id")
                    .expect("classified");
                let id = match eval_expr(id_expr, params, caller)?.gate {
                    CreateVal::Id(v) | CreateVal::Text(v) => v,
                    CreateVal::Null => {
                        return Err(
                            "null_key_address: `id: null` addresses no row — a create is \
                             spelled by leaving the id out"
                                .into(),
                        )
                    }
                    other => {
                        return Err(format!("param_type_mismatch: `id` is an id, got {other:?}"))
                    }
                };
                let rest: Vec<(ordo_syntax::Ident, Expr)> = fields
                    .iter()
                    .filter(|(n, _)| n.name != "id")
                    .cloned()
                    .collect();
                if rest.iter().all(|(n, _)| n.name == "rev") {
                    return Err(format!(
                        "invalid_binding_expr: `set {entity} {{ id: … }}` describes no field \
                         — a converge makes described state true, and this describes none"
                    ));
                }
                self.plan_addressed_set(db, log, entity, &id, &rest, params, caller)
            }
            SetMode::Create => {
                // Rule 9's last line: `rev` states a belief about an
                // addressed row, and this statement addresses none.
                if fields.iter().any(|(n, _)| n.name == "rev") {
                    return Err(format!(
                        "invalid_binding_expr: `rev` states a belief about ONE addressed row, \
                         and `set {entity} {{ … }}` addresses none — add the unique key the \
                         belief is about, or drop the `rev`"
                    ));
                }
                self.plan_insert(db, log, entity, fields, params, caller, None, group)
            }
            SetMode::Keyed { key, .. } => {
                self.plan_upsert(db, log, entity, &key, fields, params, caller, group)
            }
        }
    }

    /// **Converge-or-create** (§6 rule 8, the one blessed upsert), and
    /// rule 9's keyed beliefs on top of it.
    ///
    /// The shape is: resolve the addressing key, probe for the row it
    /// names **under a row lock**, and let that decide which of the two
    /// branches' preconditions run. The mutation itself is one
    /// `INSERT … ON CONFLICT` — `implementation.md` pins it, and the pin
    /// matters: a `SELECT` that finds nothing followed by an `INSERT` is a
    /// race with every other writer holding the same key, and losing it
    /// means either a duplicate the unique index refuses loudly or a
    /// silent second row where the whole point was one.
    ///
    /// ## Which guard applies, and when
    ///
    /// The branches ask **different authorization questions**, so the
    /// branch is decided first and exactly one guard runs:
    ///
    /// - The **create** branch asks the create-side question (G10/G10a,
    ///   `create_write_query`): *may this caller bring a row with THESE
    ///   VALUES into existence?* It has to be evaluated against the
    ///   would-be row, because there is no row yet.
    /// - The **converge** branch asks the write guard's question: *may
    ///   this caller change THIS EXISTING ROW?* — evaluated against the
    ///   row as stored, which holds values the caller never supplied (a
    ///   different parent, a different owner) and is precisely what the
    ///   guard is about.
    ///
    /// Running the create-side gate on a converge would be an escalation:
    /// it authorizes against values the CALLER chose, so anyone able to
    /// create a row could converge onto someone else's row by supplying
    /// values that satisfy the create gate. Running the write guard on a
    /// create is not even defined — there is no row to evaluate it
    /// against. Hence: decide, then check the branch's own guard.
    ///
    /// What is left is the window the pinned recipe deliberately leaves to
    /// the database: another transaction may insert this key between our
    /// probe and our `INSERT`, so the branch the database takes can differ
    /// from the branch we authorized. That is why this statement also owes
    /// a **post-image obligation located by the key** ([`PostRow::Key`]):
    /// whichever row ends up holding it must be inside the caller's write
    /// view, or the group is refused and (on the transactional path) rolls
    /// back. A race can cost a caller their write; it cannot hand them one
    /// they were not entitled to.
    #[allow(clippy::too_many_arguments)]
    fn plan_upsert(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        entity: &str,
        key: &[String],
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
        group: &[(String, String)],
    ) -> Result<PlannedStmt, String> {
        let e = self.entity_for_write(entity)?;
        // `rev` is a belief, never an assignment — split before anything
        // reads the braces as values (§6 rule 9). `addressed: false`: this
        // statement addresses by KEY, and asserting the absence of a key
        // is coherent in a way asserting the absence of an id is not.
        let (belief, fields) = self.split_rev(fields, params, caller, false)?;
        let fields = fields.as_slice();

        // A composite key is resolved column by column and addresses as a
        // whole: every component is part of one identity claim, so one
        // null component makes the whole claim unaddressed, exactly as a
        // null single-column key does.
        let mut columns = Vec::with_capacity(key.len());
        let mut literals = Vec::with_capacity(key.len());
        for k in key {
            let f = e.field(k).expect("classified from the schema").clone();
            let (_, key_expr) = fields
                .iter()
                .find(|(n, _)| &n.name == k)
                .expect("classified from the braces");
            let key_val = eval_expr(key_expr, params, caller)?;
            // Rule 3's null clause: `null == null` is true in this
            // language, so a null key is indistinct from every other
            // unaddressed row — it is not an identity claim, and
            // Postgres's default `NULLS DISTINCT` would quietly make it a
            // second row under the one blessed upsert.
            if matches!(key_val.gate, CreateVal::Null) {
                return Err(format!(
                    "null_key_address: `{entity}.{k}` is null, which addresses no row — a \
                     create is spelled by leaving the key out"
                ));
            }
            let (column, literal) = self.key_address(&e, &f, &key_val)?;
            columns.push(column);
            literals.push(literal);
        }

        // The probe, under `FOR UPDATE`. On the transactional path this
        // locks the row it finds, so the branch decision cannot be undone
        // beneath us by a concurrent converge or delete; the one case it
        // cannot lock is a row that does not exist yet, which is the case
        // `ON CONFLICT` resolves.
        let existing = self.row_by_key(db, entity, &columns, &literals, true)?;

        self.plan_insert(
            db,
            log,
            entity,
            fields,
            params,
            caller,
            Some(Upsert {
                key: key.to_vec(),
                columns,
                literals,
                existing,
                belief,
            }),
            group,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_insert(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        entity: &str,
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
        upsert: Option<Upsert>,
        // `(entity, id)` for every row an EARLIER statement of this
        // command group creates. DL §3: "statements in a command group
        // already execute in order" — so a row created by statement 1
        // EXISTS for statement 2, and the parent-visibility check below
        // must see it. It cannot see it in the database: the harness plans
        // every statement before applying any of them (AA-004's staging
        // gap), so the parent's INSERT has not run.
        //
        // Admitting it here is not a hole in G12. The create that made the
        // row proved its own authorization at its own statement — a caller
        // who could not create it never reaches this line, because the
        // group aborts at the first refusal — and no row from outside the
        // group is admitted. The check is unchanged for every write that
        // names a parent it did not just make.
        group: &[(String, String)],
    ) -> Result<PlannedStmt, String> {
        let e = self
            .schema
            .entity(entity)
            .ok_or_else(|| format!("unknown_source: no entity `{entity}`"))?
            .clone();
        self.refuse_unwritable(&e)?;

        // Supplied values.
        let mut vals: BTreeMap<String, EvaledVal> = BTreeMap::new();
        for (name, expr) in fields {
            let f = e.field(&name.name).ok_or_else(|| {
                format!(
                    "projection_unknown_field: `{entity}` has no field `{}`",
                    name.name
                )
            })?;
            if f.implicit {
                return Err(format!(
                    "reserved_derived_name: `{}` is written by the platform, never by an op",
                    name.name
                ));
            }
            // A converge-or-create's create half reads each field as its
            // default (2026-10-31): `n + 1` reads `0 + 1` here, while
            // the `DO UPDATE` half keeps `n + 1` against the row.
            if upsert.is_some() {
                if let Rhs::RowReading(sql) =
                    self.classify_rhs(&e, f, expr, params, caller, "creates-or-converges")?
                {
                    let v = self.create_branch_literal(db, &e, f, &sql, expr.span(), caller)?;
                    vals.insert(name.name.clone(), eval_expr(&v, params, caller)?);
                    continue;
                }
            }
            vals.insert(name.name.clone(), eval_expr(expr, params, caller)?);
        }
        // Defaults for the rest. A required field with neither a value nor
        // a default is now UNREACHABLE for a registered op: insert-
        // completeness (§6 rule 2) is a registration gate
        // (`ordo_plan::writes::check_writes`), so the statement never
        // deploys. The arm stays as a backstop for callers that plan a
        // statement without registering it, and it still refuses before
        // any SQL runs — a NOT NULL violation is the database answering a
        // question the compiler already answered.
        for f in &e.fields {
            if f.implicit || vals.contains_key(&f.name) {
                continue;
            }
            if let Some(d) = &f.default {
                // A money default is written in major units; the column
                // holds minor units (27ak). The schema build refused one
                // that does not fit, so this conversion cannot fail.
                let d = match ordo_schema::stored_numeric(f.ty.value_ty(), &fold_neg_lit(d)) {
                    Ok(Some(stored)) => stored,
                    _ => d.clone(),
                };
                vals.insert(f.name.clone(), eval_expr(&d, &[], caller)?);
            } else if f.optional {
                vals.insert(
                    f.name.clone(),
                    EvaledVal {
                        gate: CreateVal::Null,
                        sql: "NULL".into(),
                    },
                );
            } else {
                return Err(format!(
                    "unbound_required_param: create `{entity}` is missing `{}` (no default)",
                    f.name
                ));
            }
        }

        // A `json` field's value is checked to be a document and cast to
        // `jsonb`, once the defaults are in (28b).
        for f in &e.fields {
            if let Some(v) = vals.remove(&f.name) {
                vals.insert(f.name.clone(), fit_json(entity, f, v)?);
            }
        }

        // Closed value sets, checked once the defaults are in: a default
        // outside its own field's ladder is as wrong as a supplied value
        // outside it, and rarer, so it is worth catching in the same
        // place rather than trusting the derivation twice.
        for f in &e.fields {
            if let Some(v) = vals.get(&f.name) {
                check_enum(entity, f, &v.gate)?;
                check_text_max(entity, f, &v.gate)?;
            }
        }

        // The id this create will hold, minted before anything is sealed
        // because a sealed value is bound to the row it lives on: the seal
        // below names it.
        let id = format!("{}_w{}", e.key, log.mint());

        // F4: sealed fields store an AEAD envelope, never plaintext; an
        // indexed one also stores its blind digest (over the NORMALIZED
        // value — the read side digests the same way, which is the whole
        // equality story). The plaintext seals as written; normalization
        // shapes only the index.
        let mut bidx: BTreeMap<String, String> = BTreeMap::new();
        // The envelope each sealed field ends up holding, kept beside the
        // SQL literal because the image needs the BYTES: a handle names a
        // payload the store keeps, so the payload has to survive the trip
        // from "sealed here" to "imaged there".
        let mut sealed_bytes: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for f in &e.fields {
            if !f.sealed {
                continue;
            }
            let Some(v) = vals.get_mut(&f.name) else {
                continue;
            };
            let plain = match &v.gate {
                CreateVal::Text(t) | CreateVal::Id(t) => t.clone(),
                // A null PAYLOAD is a null column — there is nothing to
                // seal. Its blind INDEX is not null: it digests the
                // reserved null sentinel, or `sealed indexed unique`
                // constrains nothing (Postgres UNIQUE admits unlimited
                // NULLs) and equality against a null matches nothing.
                CreateVal::Null => {
                    if f.indexed {
                        let digest = self
                            .keys
                            .blind_index(
                                self.space(),
                                entity,
                                &f.name,
                                ordo_crypto::BlindValue::Null,
                            )
                            .map_err(|e| format!("blind index: {e}"))?;
                        bidx.insert(
                            f.index_column(),
                            format!("'\\x{}'", ordo_crypto::hex(&digest)),
                        );
                    }
                    continue;
                }
                other => {
                    return Err(format!(
                        "invalid_binding_expr: sealed `{}` takes text, got {other:?}",
                        f.name
                    ))
                }
            };
            let lease = self
                .keys
                .release(self.space(), ordo_crypto::Purpose::Write, "write")
                .map_err(|e| format!("key release: {e}"))?;
            let sealed = self
                .keys
                .seal_at(
                    &lease,
                    entity,
                    &f.name,
                    ordo_crypto::Anchor::Row(&id),
                    plain.as_bytes(),
                )
                .map_err(|e| format!("seal: {e}"))?;
            if f.indexed {
                let normalized = match f.normalize.as_deref() {
                    Some("lower") => plain.to_lowercase(),
                    _ => plain.clone(),
                };
                let digest = self
                    .keys
                    .blind_index(
                        self.space(),
                        entity,
                        &f.name,
                        ordo_crypto::BlindValue::Text(&normalized),
                    )
                    .map_err(|e| format!("blind index: {e}"))?;
                bidx.insert(
                    f.index_column(),
                    format!("'\\x{}'", ordo_crypto::hex(&digest)),
                );
            }
            *v = EvaledVal {
                // Guards never walk sealed fields; the gate sees nothing.
                gate: CreateVal::Null,
                sql: format!("'\\x{}'", ordo_crypto::hex(&sealed)),
            };
            sealed_bytes.insert(f.name.clone(), sealed);
        }

        // ── the branch's own preconditions ──────────────────────────────
        // Exactly one of these runs; which one is decided by the probe,
        // under the row lock it took. See `plan_upsert` for why the two
        // guards are not interchangeable.
        let converging = upsert.as_ref().and_then(|u| u.existing.clone());
        match &converging {
            None => {
                // G12 for creates: every supplied parent must be
                // READ-visible.
                //
                // Which values are ids is decided by the SCHEMA — this
                // field is a ref, so its value is an id however it is
                // spelled. Deciding it from the string's shape instead
                // (the old heuristic: contains `_`, under 40 characters,
                // no spaces) let a caller opt out of this very check by
                // choosing an id that did not look like one.
                for f in &e.fields {
                    let FieldTy::Ref { target, .. } = &f.ty else {
                        continue;
                    };
                    let Some(v) = vals.get(&f.name) else { continue };
                    let p = match &v.gate {
                        CreateVal::Id(p) | CreateVal::Text(p) => p,
                        // A null ref names no parent; anything else is a
                        // type error the binder will name.
                        _ => continue,
                    };
                    if !group.iter().any(|(en, id)| en == target && id == p)
                        && !self.row_visible(db, target, p, caller, false)?
                    {
                        return Err(NOT_FOUND.to_string());
                    }
                }

                // G10 against the would-be row (or G10a's exemption).
                let gate_vals: BTreeMap<String, CreateVal> = vals
                    .iter()
                    .map(|(k, v)| (k.clone(), v.gate.clone()))
                    .collect();
                match create_write_query(&self.schema, entity, &gate_vals, caller)
                    .map_err(|er| format!("bind: {er}"))?
                {
                    CreateGate::Exempt => {}
                    CreateGate::Check(q) => {
                        let sql = plan(&q, &self.plan_space())
                            .map_err(|er| format!("plan: {er}"))?
                            .sql;
                        if db.rows(&sql)?.is_empty() {
                            return Err(format!("access_guard: create `{entity}` as {caller}"));
                        }
                    }
                }
                // Rule 9's second belief, on the branch it is about: the
                // caller said "the row is at version r" and the key names
                // no row at all. Nothing to converge, and creating would
                // ignore the belief rather than answer it — so the answer
                // is the one a stale belief always gets, and the remedy is
                // the one it always has: refetch.
                if let Some(u) = &upsert {
                    if let Some(Some(want)) = u.belief {
                        return Err(format!(
                            "cas_miss: no `{entity}` holds `{}`, so it is not at rev {want}",
                            u.name()
                        ));
                    }
                }
            }
            Some(id) => {
                let u = upsert.as_ref().expect("converging implies an upsert");
                // A row the caller cannot READ answers `duplicate`: the
                // key is taken, which is exactly what the unique index
                // would have told them if we had tried the create, and
                // nothing more. Saying `access_guard` here would confirm
                // the row is one they may see the shape of; saying
                // `not_found` would claim the write addressed something
                // absent, when the whole reason it cannot proceed is that
                // the row is present.
                if !self.row_visible(db, entity, id, caller, false)? {
                    return Err(format!(
                        "duplicate: `{entity}.{}` is already taken",
                        u.name()
                    ));
                }
                // Visible but unwritable is the addressed path's answer,
                // for the addressed path's reason (G10).
                if !self.row_visible(db, entity, id, caller, true)? {
                    return Err(format!(
                        "access_guard: converge `{id}` ({entity}) as {caller}"
                    ));
                }
                // Beliefs come after authorization, as on the addressed
                // path: a caller who may not write the row learns that,
                // not what version it is at.
                match u.belief {
                    // `rev: null` — "there is no such row". There is one.
                    // Nothing is applied, and the class says which belief
                    // failed rather than blaming the data (`duplicate`
                    // is a constraint the caller did not anticipate;
                    // this one they stated and the data refused).
                    Some(None) => {
                        return Err(format!(
                            "expected_absent: `{entity}.{}` already names `{id}`, and this \
                             write asserted there was no such row",
                            u.name()
                        ))
                    }
                    Some(Some(want)) => self.check_cas(db, entity, id, want)?,
                    None => {}
                }
                self.check_reassignment(db, &e, fields, params, caller)?;
            }
        }

        // The converge half of the upsert, described-fields only.
        // The converge half seals at the row the probe resolved. That is
        // the whole of the upsert answer: the key is resolved first, under
        // the row lock, and the value is sealed second, at the id the
        // database gave it. When the probe found no row the converge arm
        // can only run if a concurrent insert took the key after the probe,
        // and that write is refused below (`expected_absent`, which rolls
        // the group back), so the id it seals at, this create's own, never
        // reaches a row it does not belong to.
        let sets = match &upsert {
            None => None,
            Some(_) => {
                let at = converging.as_deref().unwrap_or(&id);
                Some(self.assignments(&e, fields, params, caller, "converges", at)?)
            }
        };

        // Insert, image. The id was minted above, ahead of the seals.
        // **The create stamps the space.** It is the platform's column, so
        // it is written here and nowhere a caller can reach: there is no
        // field named `$space`, no brace that assigns it, and no default
        // that could be overridden. A row's tenancy is decided by the door
        // that bound the request, once, at the moment the row comes into
        // existence — and it never changes afterwards, because no
        // statement this path emits ever assigns it.
        let mut cols = vec![q_ident(ordo_schema::SPACE_COLUMN), q_ident("id")];
        let mut lits = vec![self.space_lit(), sql_str(&id)];
        let mut image: Image = BTreeMap::new();
        image.insert("id".into(), LogValue::Id(id.clone()));
        image.insert("rev".into(), LogValue::Int(1));
        for f in &e.fields {
            if f.implicit {
                continue;
            }
            let v = vals.get(&f.name).expect("filled above");
            cols.push(q_ident(&f.column()));
            lits.push(v.sql.clone());
            if f.sealed {
                // The image carries an opaque handle, never the value —
                // and the handle now names a payload the store holds.
                // It used to be counter-derived, which was honest about
                // not being random and dishonest about resolving: a
                // handle nothing persists is a dangling pointer in the
                // audit spine.
                match sealed_bytes.get(&f.name) {
                    None => image.insert(f.name.clone(), LogValue::Null),
                    Some(bytes) => image.insert(
                        f.name.clone(),
                        self.seal_image_ref(
                            log,
                            entity,
                            &f.name,
                            &ordo_crypto::Anchor::Row(&id).label(),
                            bytes,
                        )?,
                    ),
                };
                if f.indexed {
                    // Never `unwrap_or("NULL")`. The digest is TOTAL now —
                    // a null digests the sentinel — so a missing entry is a
                    // bug in the loop above, and defaulting to SQL NULL is
                    // how the constraint quietly stopped constraining. The
                    // generated DDL declares `$bidx` NOT NULL, so this
                    // would refuse at the database anyway; refusing here
                    // says which field.
                    cols.push(q_ident(&f.index_column()));
                    lits.push(bidx.get(&f.index_column()).cloned().ok_or_else(|| {
                        format!(
                            "internal: no blind digest computed for `{entity}.{}`",
                            f.name
                        )
                    })?);
                }
            } else {
                image.insert(f.name.clone(), log_value(&v.gate));
            }
        }
        cols.push(q_ident("rev"));
        lits.push("1".into());
        // A tree row's path and depth are the platform's to write: they are
        // derived from the parent chain, never supplied, so there is no
        // spelling by which a caller can claim a position in the tree it
        // did not earn through a real parent.
        if e.is_tree {
            if let Some((field, _)) = self.tree_ref(&e) {
                let parent = match vals.get(&field).map(|v| &v.gate) {
                    Some(CreateVal::Id(p)) | Some(CreateVal::Text(p)) => Some(p.clone()),
                    _ => None,
                };
                let (path, depth) = self.path_under(db, entity, parent.as_deref(), &id)?;
                cols.push(q_ident("$path"));
                lits.push(sql_str(&path));
                cols.push(q_ident("depth"));
                lits.push(depth.to_string());
                image.insert("depth".into(), LogValue::Int(depth));
            }
        }
        // ── the mutation ────────────────────────────────────────────────
        // A plain create is a plain INSERT. A converge-or-create is the
        // pinned recipe: ONE `INSERT … ON CONFLICT` on the addressing key,
        // so the branch is taken by the database against the unique index
        // rather than by us against a `SELECT` that is already stale by
        // the time the INSERT runs. The INSERT half carries the whole row
        // (supplied values plus defaults — rule 2's insert-completeness is
        // what makes that possible); the `DO UPDATE` half carries only the
        // described fields (rule 10: a converge touches what it describes,
        // and defaults belong to the create branch alone).
        let conflict = upsert.as_ref().map(|u| {
            // The inference clause names the index's columns in the
            // index's own order: the space, then the key. A composite key
            // contributes all of its columns, because that is the index
            // `unique (board, member)` built and Postgres matches an
            // arbiter by its column set, not by a prefix of it.
            let arbiter = std::iter::once(q_ident(ordo_schema::SPACE_COLUMN))
                .chain(u.columns.iter().map(|c| q_ident(c)))
                .collect::<Vec<_>>()
                .join(", ");
            if u.create_once() {
                // Create-once must not fall into a converge, so the
                // conflict action is DO NOTHING. If a concurrent insert
                // took the key between the probe and here, this statement
                // applies nothing and the keyed post-image obligation
                // turns that into `expected_absent` — the answer the
                // caller's own belief earns.
                // The addressing key's unique index is `(space, key)`, so
                // the inference clause has to name both or Postgres finds
                // no matching index at all. It is also the correct
                // semantics: a key taken in ANOTHER space is not a
                // conflict, and this statement must create.
                format!("\nON CONFLICT ({arbiter}) DO NOTHING")
            } else {
                format!(
                    "\nON CONFLICT ({arbiter}) DO UPDATE SET {}, {rev} = {t}.{rev} + 1",
                    sets.as_deref().unwrap_or(""),
                    t = q_ident(entity),
                    rev = q_ident("rev"),
                )
            }
        });
        let mut sql = vec![format!(
            "INSERT INTO {} ({}) VALUES ({}){};",
            q_ident(entity),
            cols.join(", "),
            lits.join(", "),
            conflict.unwrap_or_default(),
        )];
        // Converging onto a tree node can move it, and a move rewrites
        // this node's path and every descendant's — the same maintenance
        // the addressed converge owes, for the same reason.
        if let Some(existing) = &converging {
            if let Some(extra) = self.reparent_sql(db, &e, existing, fields, params, caller)? {
                sql.push(extra);
            }
        }
        let mut commands = vec![match &converging {
            None => PendingCommand {
                entity_name: entity.to_string(),
                row_id: id.clone(),
                kind: CommandKind::Create,
                before: None,
                after: Some(image),
            },
            Some(existing) => PendingCommand {
                entity_name: entity.to_string(),
                row_id: existing.clone(),
                kind: CommandKind::Converge,
                before: Some(
                    self.row_image(db, log, entity, existing)?
                        .ok_or_else(not_found)?,
                ),
                after: None, // taken post-batch
            },
        }];

        // The acl bootstrap: the creator's top-level access row rides the
        // same command group, or the exempt create mints an orphan no one
        // can ever administer. It belongs to the CREATE branch only — a
        // converge lands on a row that already has its access rows, and
        // minting another would grant the caller a level nobody gave them.
        //
        // **The auto-grant follows space MEMBERSHIP, not authorship**
        // (spec §3 bootstrap rules 4 and 5, R-G11a, GC092). Three cases,
        // and the create succeeds in all three:
        //
        // - a **member** — a `members` row with a role — receives the top
        //   level, minted in this same group;
        // - a **guest** — a `members` row with a NULL role, bound to the
        //   space through an inter-space share — receives nothing;
        // - a **routine, system, or source** principal — no `members` row
        //   at all — receives nothing, because there is no human to grant
        //   to and inventing one is the platform declaring consent.
        //
        // The create is **not** refused for a guest, and the difference
        // matters. R-G11a's exemption is about the guard: `boards`' write
        // guard is self-rooted, so it is not evaluated on a create, and
        // that is true whoever the caller is. What bounds the create is
        // the door — may this principal call `openBoard` at all — and the
        // op body. Refusing here would make the auto-grant rule into a
        // second, undeclared authorization check that the door already
        // owns, and it would refuse the routine case too, which the spec
        // explicitly expects to succeed and land an ungranted row.
        // GC092 says "applied … and zero access rows are minted".
        //
        // Such rows are not orphans: bootstrap rule 6 makes the SPACE
        // OWNER the repair path for every acl row in their space, checked
        // at the door once per call, so no arrangement of data can leave a
        // row nobody can administer.
        let acl_root = e.acl_levels.as_ref().filter(|_| converging.is_none());
        let grants = match acl_root {
            None => false,
            Some(_) => self.caller_is_space_member(db, caller)?,
        };
        if let Some(levels) = acl_root.filter(|_| grants) {
            let top = levels.last().expect("acl has levels").clone();
            let access = format!("{entity}Access");
            let back = entity.strip_suffix('s').unwrap_or(entity).to_string();
            let ae = self
                .schema
                .entity(&access)
                .ok_or_else(|| format!("derived `{access}` missing"))?;
            let aid = format!("{}_w{}", ae.key, log.mint());
            sql.push(format!(
                "INSERT INTO {} ({}, {}, {}, {}, {}, {}) VALUES ({}, {}, {}, {}, {}, 1);",
                q_ident(&access),
                q_ident(ordo_schema::SPACE_COLUMN),
                q_ident("id"),
                q_ident(&back),
                q_ident("member"),
                q_ident("level"),
                q_ident("rev"),
                self.space_lit(),
                sql_str(&aid),
                sql_str(&id),
                sql_str(caller),
                sql_str(&top),
            ));
            let mut aimg: Image = BTreeMap::new();
            aimg.insert("id".into(), LogValue::Id(aid.clone()));
            aimg.insert("rev".into(), LogValue::Int(1));
            aimg.insert(back, LogValue::Id(id.clone()));
            aimg.insert("member".into(), LogValue::Id(caller.to_string()));
            aimg.insert("level".into(), LogValue::Text(top));
            commands.push(PendingCommand {
                entity_name: access,
                row_id: aid,
                kind: CommandKind::Create,
                before: None,
                after: Some(aimg),
            });
        }

        Ok(PlannedStmt {
            summary: match &converging {
                None => json!({ "entity": entity, "created": id }),
                // Named, not counted: a converge-or-create that says
                // `updated: 1` tells the caller nothing they can act on,
                // and the row it landed on is the one fact this mode
                // exists to establish.
                Some(existing) => json!({ "entity": entity, "converged": existing }),
            },
            sql,
            commands,
            post: match &upsert {
                // A plain create already proved its guard against the
                // would-be row.
                None => Vec::new(),
                Some(u) => vec![PostRow::Key {
                    entity: entity.to_string(),
                    columns: u.columns.clone(),
                    literals: u.literals.clone(),
                    // On the create branch the row this statement is
                    // answerable for is the one it minted; if the key
                    // ends up on a different row, a concurrent insert
                    // won the race.
                    expect: converging.is_none().then(|| id.clone()),
                }],
            },
        })
    }

    // ── addressed ───────────────────────────────────────────────────────

    // Eight arguments, and the eighth is the write log. It is here because
    // a sealed field's before-image mints a handle and stages the payload
    // that handle names, and the log is where a staged payload waits for
    // its group's correlation id. Bundling these into a context struct
    // would hide which of them the audit-scoped `db` is, which is the one
    // argument that must not be swapped by accident.
    #[allow(clippy::too_many_arguments)]
    fn plan_addressed_set(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        entity: &str,
        id: &str,
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
    ) -> Result<PlannedStmt, String> {
        let e = self.entity_for_write(entity)?;
        // Lock the addressed row BEFORE its guards are evaluated: under a
        // transaction this makes the precondition and the mutation one
        // decision, so a concurrent write cannot slip between "the guard
        // held" and "the update ran" (AA-004). A row that does not exist
        // locks nothing and falls through to the not_found below.
        self.lock_row(db, entity, id)?;
        // G12 then G10 — the order IS the disclosure rule: a row outside
        // the read view must fail exactly like an absent row.
        if !self.row_visible(db, entity, id, caller, false)? {
            return Err(NOT_FOUND.to_string());
        }
        if !self.row_visible(db, entity, id, caller, true)? {
            return Err(format!("access_guard: write `{id}` as {caller}"));
        }
        // The stated belief is checked AFTER the guards and under the same
        // row lock: a caller who may not write the row learns that first,
        // and a caller who may gets a version answer nothing can race.
        let (belief, fields) = self.split_rev(fields, params, caller, true)?;
        let fields = fields.as_slice();
        if let Some(Some(want)) = belief {
            self.check_cas(db, entity, id, want)?;
        }
        self.check_reassignment(db, &e, fields, params, caller)?;
        self.check_conflicting_identity(db, &e, id, fields, params, caller)?;
        let before = self.row_image(db, log, entity, id)?.ok_or_else(not_found)?;
        let sets = self.assignments(&e, fields, params, caller, "converges", id)?;
        let mut sql = format!(
            "UPDATE {} SET {}, {rev} = {rev} + 1 WHERE {sp} AND {} = {};",
            q_ident(entity),
            sets,
            q_ident("id"),
            sql_str(id),
            sp = self.space_pred(),
            rev = q_ident("rev"),
        );
        // Reparenting rewrites this node's path and every descendant's,
        // because a descendant's path CONTAINS its ancestors' ids. Doing it
        // as one prefix substitution keeps the subtree consistent at every
        // depth without walking it row by row.
        if let Some(extra) = self.reparent_sql(db, &e, id, fields, params, caller)? {
            sql.push('\n');
            sql.push_str(&extra);
        }
        Ok(PlannedStmt {
            summary: json!({ "entity": entity, "updated": 1 }),
            sql: vec![sql],
            commands: vec![PendingCommand {
                entity_name: entity.to_string(),
                row_id: id.to_string(),
                kind: CommandKind::Converge,
                before: Some(before),
                after: None, // taken post-batch
            }],
            post: vec![PostRow::Id {
                entity: entity.to_string(),
                id: id.to_string(),
            }],
        })
    }

    fn plan_addressed_del(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        entity: &str,
        id: &str,
        caller: &str,
    ) -> Result<PlannedStmt, String> {
        let e = self.entity_for_write(entity)?;
        self.lock_row(db, entity, id)?;
        // **Authorization first, and this order is load-bearing.** These
        // two checks used to run *after* `refuse_tree_parent` and
        // `refuse_with_referrers`, and that was an enumeration oracle of
        // exactly the shape R-G10 closes: a row the caller may not read but
        // which has children or referrers answered `tree_has_children` /
        // `has_referrers`, while an id that never existed has neither and
        // fell through to `not_found`. Two ids, two classes, and the
        // difference is a fact about a row the caller cannot see.
        //
        // The structural refusals are answers about the shape of the data,
        // so they may only be given to a caller who has been told the row is
        // there. `plan_subtree_del` already had this right; the addressed
        // delete did not.
        if !self.row_visible(db, entity, id, caller, false)? {
            return Err(NOT_FOUND.to_string());
        }
        if !self.row_visible(db, entity, id, caller, true)? {
            return Err(format!("access_guard: delete `{id}` as {caller}"));
        }
        self.refuse_tree_parent(db, &e, id)?;
        let roots = [(entity.to_string(), id.to_string())];
        let victims = self.owned_cascade(db, &roots)?;
        self.lock_cascaded(db, &victims, &roots)?;
        self.refuse_unwritable_cascade(db, &victims, &roots, caller)?;
        self.refuse_plain_referrers(db, &victims)?;
        let victims = self.delete_order(db, victims)?;
        let (commands, sql) = self.cascade_delete_plan(db, log, &victims)?;
        Ok(PlannedStmt {
            summary: json!({ "entity": entity, "deleted": 1 }),
            sql,
            commands,
            // A deleted row has no post-image to prove anything about.
            post: Vec::new(),
        })
    }

    /// `del <tree> { id: $anchor } subtree` — the explicit cascade
    /// (§tree, "Deleting tree parents" [settled]).
    ///
    /// Three properties the spelling promises, all of them enforced here:
    ///
    /// 1. **Per-row commands, so revertible.** One `Delete` command with
    ///    its own `before` image per deleted row, never one bulk command
    ///    for the cascade — a revert has to be able to put the rows back,
    ///    and it can only put back what it has images of.
    /// 2. **Guards are evaluated over the whole subtree, not the anchor
    ///    alone** (engine-plan §write). See [`Self::refuse_partial_subtree`]
    ///    for the all-or-nothing ruling.
    /// 3. **Referential integrity still holds.** A row OUTSIDE the subtree
    ///    that points into it blocks the cascade exactly as it blocks a
    ///    single delete. Rule 6 excepts trees from *their own* parent ref,
    ///    not from everybody else's refs.
    ///
    /// Deepest first, throughout: locks, refusals, commands, and the
    /// `DELETE`s themselves. A child's parent ref points at the row above
    /// it, so a parent deleted before its children is a foreign-key
    /// violation; `depth DESC` is that order, straight from the column the
    /// tree already maintains.
    fn plan_subtree_del(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        entity: &str,
        anchor: &str,
        caller: &str,
    ) -> Result<PlannedStmt, String> {
        let e = self.entity_for_write(entity)?;
        if self.tree_ref(&e).is_none() {
            // Static: `subtree` on a non-tree entity is a malformed op,
            // caught when the op is planned rather than when a caller
            // trips it. It carries no wire class deliberately — the closed
            // inventory (`writing-data.md` §7) names what a *deployed* op
            // can do to a caller, and this can never reach one.
            return Err(format!(
                "`subtree` is only spellable on a tree entity, and `{entity}` has no `tree` ref"
            ));
        }
        // The anchor answers `not_found` before anything else, exactly as
        // the addressed delete does: a row outside your read view is a row
        // that is not there (G12).
        if !self.row_visible(db, entity, anchor, caller, false)? {
            return Err(NOT_FOUND.to_string());
        }
        let (path, _) = self.node_path(db, entity, anchor)?.ok_or_else(not_found)?;
        // The subtree is exactly the rows whose `$path` has the anchor's
        // `$path` as a prefix — the anchor itself included, since its own
        // path prefixes itself. The trailing delimiter is what makes this
        // exact: `cat_1/` never prefixes `cat_10/`.
        let victims: Vec<String> = first_col(db.rows(&format!(
            "SELECT {id} FROM {t} WHERE {sp} AND left({p}, {n}) = {a} ORDER BY {d} DESC, {id}",
            id = q_ident("id"),
            t = q_ident(entity),
            sp = self.space_pred(),
            p = q_ident("$path"),
            n = path.len(),
            a = sql_str(&path),
            d = q_ident("depth"),
        ))?);
        // Lock in delete order, so two concurrent cascades over
        // overlapping subtrees queue rather than deadlock.
        for id in &victims {
            self.lock_row(db, entity, id)?;
        }
        self.refuse_partial_subtree(db, entity, anchor, &victims, caller)?;
        // Each tree row takes the rows it owns with it. The tree rows are
        // roots of the cascade, so a row goes only after everything that
        // references it.
        let roots: Vec<(String, String)> = victims
            .iter()
            .map(|id| (entity.to_string(), id.clone()))
            .collect();
        let doomed = self.owned_cascade(db, &roots)?;
        self.lock_cascaded(db, &doomed, &roots)?;
        self.refuse_unwritable_cascade(db, &doomed, &roots, caller)?;
        // Referrers from inside the cascade are going away with it, so
        // they do not block; anything else does. Note the asymmetry with
        // `refuse_tree_parent`, which is NOT run here: cascading past a
        // node's own children is the whole point of the word.
        self.refuse_plain_referrers(db, &doomed)?;
        let doomed = self.delete_order(db, doomed)?;
        let (commands, sql) = self.cascade_delete_plan(db, log, &doomed)?;
        Ok(PlannedStmt {
            summary: json!({ "entity": entity, "deleted": victims.len() }),
            sql,
            commands,
            // Deleted rows have no post-image to prove anything about.
            post: Vec::new(),
        })
    }

    /// A subtree cascade is **all or nothing**: if any row under the
    /// anchor is outside the caller's write view, the whole statement is
    /// refused.
    ///
    /// The spec settles the input — `del … subtree` "evaluates guards over
    /// the whole subtree, not the anchor alone" (engine-plan §write) — but
    /// not what to do when only some rows pass, so this is the harness's
    /// ruling and the safe one. Deleting the reachable part and leaving
    /// the rest would leave a forest of orphans whose parent no longer
    /// exists: rows still holding a `parent` id that points at nothing,
    /// paths and depths describing a tree that is gone, and no way for the
    /// caller — who by construction cannot see or write those rows — to
    /// repair it. The bulk rule (G11: rows outside the write view are
    /// silently outside the range) is deliberately NOT applied here, and
    /// the difference is structural: a bulk delete's rows are independent,
    /// so narrowing the range leaves every survivor coherent, while a
    /// subtree is one structure and half of it is not a smaller structure.
    ///
    /// The refusal names the anchor and the count, never the ids: a caller
    /// who cannot see a descendant does not learn which rows exist from
    /// being told no.
    fn refuse_partial_subtree(
        &self,
        db: &dyn WriteDb,
        entity: &str,
        anchor: &str,
        victims: &[String],
        caller: &str,
    ) -> Result<(), String> {
        if !self.row_visible(db, entity, anchor, caller, true)? {
            // The anchor's own failure is the ordinary single-row refusal;
            // nothing about the subtree makes it a different answer.
            return Err(format!("access_guard: delete `{anchor}` as {caller}"));
        }
        let mut blocked = 0;
        for id in victims {
            if id != anchor && !self.row_visible(db, entity, id, caller, true)? {
                blocked += 1;
            }
        }
        if blocked > 0 {
            return Err(format!(
                "access_guard: the subtree under `{anchor}` ({entity}) holds {blocked} row{} \
                 you may not delete — a partial cascade would orphan the rest, so none of it \
                 is deleted",
                if blocked == 1 { "" } else { "s" }
            ));
        }
        Ok(())
    }

    // ── bulk ────────────────────────────────────────────────────────────

    // Eight arguments, and the eighth is the write log. It is here because
    // a sealed field's before-image mints a handle and stages the payload
    // that handle names, and the log is where a staged payload waits for
    // its group's correlation id. Bundling these into a context struct
    // would hide which of them the audit-scoped `db` is, which is the one
    // argument that must not be swapped by accident.
    #[allow(clippy::too_many_arguments)]
    fn plan_bulk(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        entity: &str,
        cond: &Condition,
        fields: Option<&[(ordo_syntax::Ident, Expr)]>,
        params: &[(String, Expr)],
        caller: &str,
    ) -> Result<PlannedStmt, String> {
        let e = self.entity_for_write(entity)?;
        // The range: read view ∩ condition, through the ordinary read
        // binder so the guard, `$caller`, and the params all mean exactly
        // what they mean in a read.
        let span = Span::new(0, 0, 1, 1);
        let pl = Pipeline {
            lets: Vec::new(),
            sources: vec![Source {
                name: ordo_syntax::Ident {
                    name: entity.to_string(),
                    span,
                },
                args: vec![SourceArg::Cond(cond.clone())],
                projection: Some(vec![ProjItem::Field {
                    path: vec![ordo_syntax::Ident {
                        name: "id".into(),
                        span,
                    }],
                    span,
                }]),
                span,
            }],
            steps: Vec::new(),
            span,
        };
        let q = bind_as(
            &self.schema,
            &self.registry,
            &pl,
            params,
            Some(caller),
            ordo_sema::Clock::CORPUS,
        )
        .map_err(|er| format!("bind: {er}"))?;
        let sel_sql = plan(&q, &self.plan_space())
            .map_err(|er| format!("plan: {er}"))?
            .sql;
        let selected: Vec<String> = first_col(db.rows(&sel_sql)?);

        // ∩ write view (G11): rows the caller cannot write are silently
        // outside the range.
        let writable: Vec<String> =
            match guard_filter_query(&self.schema, entity, true, Some(caller))
                .map_err(|er| format!("bind: {er}"))?
            {
                None => selected.clone(),
                Some(wq) => {
                    let wsql = plan(&wq, &self.plan_space())
                        .map_err(|er| format!("plan: {er}"))?
                        .sql;
                    let wids = first_col(db.rows(&wsql)?);
                    selected.into_iter().filter(|i| wids.contains(i)).collect()
                }
            };

        if let Some(fs) = fields {
            if fs.iter().any(|(n, _)| n.name == "rev") {
                return Err(
                    "invalid_binding_expr: `rev` states a belief about ONE row; a ranged \
                     write addresses many, so there is no version to believe in"
                        .into(),
                );
            }
            self.check_reassignment(db, &e, fs, params, caller)?;
            // A ranged reparent would need a per-row path rewrite, and the
            // rows can be at different depths with overlapping subtrees —
            // the substitutions would interfere. Refused rather than
            // half-maintained: `set <tree>(id == …) { parent: … }` moves
            // one node, which is the shape the settled semantics describe.
            if let Some((tref, _)) = self.tree_ref(&e) {
                if fs.iter().any(|(n, _)| n.name == tref) {
                    return Err(format!(
                        "tree_bulk_reparent: move `{}` nodes one at a time — a ranged \
                         reparent cannot keep overlapping subtrees consistent",
                        e.name
                    ));
                }
            }
        }
        let mut sql = Vec::new();
        let mut commands = Vec::new();
        for id in &writable {
            self.lock_row(db, entity, id)?;
            if fields.is_none() {
                self.refuse_tree_parent(db, &e, id)?;
            }
        }
        // A ranged delete takes the rows each match owns with it.
        let mut doomed = Vec::new();
        if fields.is_none() {
            let roots: Vec<(String, String)> = writable
                .iter()
                .map(|id| (entity.to_string(), id.clone()))
                .collect();
            doomed = self.owned_cascade(db, &roots)?;
            self.lock_cascaded(db, &doomed, &roots)?;
            self.refuse_unwritable_cascade(db, &doomed, &roots, caller)?;
            self.refuse_plain_referrers(db, &doomed)?;
            if doomed.len() > roots.len() {
                doomed = self.delete_order(db, doomed)?;
            }
        }
        let kind = if fields.is_some() {
            CommandKind::Converge
        } else {
            CommandKind::Delete
        };
        if doomed.len() > writable.len() {
            let (commands, sql) = self.cascade_delete_plan(db, log, &doomed)?;
            return Ok(PlannedStmt {
                summary: json!({ "entity": entity, "deleted": writable.len() }),
                sql,
                commands,
                post: Vec::new(),
            });
        }
        for id in &writable {
            let before = self
                .row_image(db, log, entity, id)?
                .ok_or_else(|| format!("selected row `{id}` vanished"))?;
            commands.push(PendingCommand {
                entity_name: entity.to_string(),
                row_id: id.clone(),
                kind,
                before: Some(before),
                after: None,
            });
        }
        if !writable.is_empty() {
            let in_list = writable
                .iter()
                .map(|i| sql_str(i))
                .collect::<Vec<_>>()
                .join(", ");
            match fields {
                // A ranged write that assigns a sealed field seals once per
                // row, because the envelope is bound to the row it lands
                // on. The ids are already in hand (the range was read to
                // find them, and each row is locked and imaged above), so
                // the cost is one seal and one statement per row touched,
                // on top of work this path already does per row. A ranged
                // write that assigns no sealed field stays one statement.
                Some(fs)
                    if fs
                        .iter()
                        .any(|(n, _)| e.field(&n.name).is_some_and(|f| f.sealed)) =>
                {
                    for id in &writable {
                        sql.push(format!(
                            "UPDATE {} SET {}, {rev} = {rev} + 1 WHERE {sp} AND {} = {};",
                            q_ident(entity),
                            self.assignments(&e, fs, params, caller, "bulk", id)?,
                            q_ident("id"),
                            sql_str(id),
                            sp = self.space_pred(),
                            rev = q_ident("rev"),
                        ));
                    }
                }
                Some(fs) => sql.push(format!(
                    "UPDATE {} SET {}, {rev} = {rev} + 1 WHERE {sp} AND {} IN ({in_list});",
                    q_ident(entity),
                    self.assignments(&e, fs, params, caller, "bulk", "")?,
                    q_ident("id"),
                    sp = self.space_pred(),
                    rev = q_ident("rev"),
                )),
                None => sql.push(format!(
                    "DELETE FROM {} WHERE {sp} AND {} IN ({in_list});",
                    q_ident(entity),
                    q_ident("id"),
                    sp = self.space_pred(),
                )),
            }
        }
        let n = writable.len();
        Ok(PlannedStmt {
            summary: if fields.is_some() {
                json!({ "entity": entity, "updated": n })
            } else {
                json!({ "entity": entity, "deleted": n })
            },
            sql,
            commands,
            // Deletes have no post-image; ranged updates owe one per row.
            post: match fields {
                None => Vec::new(),
                Some(_) => writable
                    .iter()
                    .map(|id| PostRow::Id {
                        entity: entity.to_string(),
                        id: id.clone(),
                    })
                    .collect(),
            },
        })
    }

    // ── shared checks ───────────────────────────────────────────────────

    /// This write's space, as a SQL literal.
    pub(crate) fn space_lit(&self) -> String {
        sql_str(&self.space().0)
    }

    /// `"$space" = '<this write's space>'` — the conjunct every statement
    /// on this path starts its WHERE with.
    ///
    /// Unqualified, because the write path addresses one table per
    /// statement and never aliases it. The read path, which joins, gets
    /// its predicates from the planner and qualifies each by alias.
    pub(crate) fn space_pred(&self) -> String {
        format!(
            "{} = {}",
            q_ident(ordo_schema::SPACE_COLUMN),
            self.space_lit()
        )
    }

    fn entity_for_write(&self, entity: &str) -> Result<Entity, String> {
        let e = self
            .schema
            .entity(entity)
            .ok_or_else(|| format!("unknown_source: no entity `{entity}`"))?
            .clone();
        self.refuse_unwritable(&e)?;
        Ok(e)
    }

    /// Nothing is refused by class any more: tree entities maintain their
    /// materialized path and depth here (§tree, settled), and sealed
    /// fields have written since F4 — it is sealed READS that stay
    /// refused.
    fn refuse_unwritable(&self, _e: &Entity) -> Result<(), String> {
        Ok(())
    }

    /// The tree ref on this entity, if it has one: `(field, target)`.
    fn tree_ref(&self, e: &Entity) -> Option<(String, String)> {
        e.fields.iter().find_map(|f| match &f.ty {
            FieldTy::Ref {
                target, tree: true, ..
            } => Some((f.name.clone(), target.clone())),
            _ => None,
        })
    }

    /// A node's stored path and depth, read straight from the row.
    fn node_path(
        &self,
        db: &dyn WriteDb,
        entity: &str,
        id: &str,
    ) -> Result<Option<(String, i64)>, String> {
        let rows = db.rows(&format!(
            "SELECT {}, {} FROM {} WHERE {sp} AND {} = {}",
            q_ident("$path"),
            q_ident("depth"),
            q_ident(entity),
            q_ident("id"),
            sql_str(id),
            sp = self.space_pred(),
        ))?;
        Ok(rows.first().and_then(|r| {
            let p = r.first().cloned().flatten()?;
            let d = r.get(1).cloned().flatten()?.parse().ok()?;
            Some((p, d))
        }))
    }

    /// The path and depth a node takes under `parent` (None = a root).
    ///
    /// The path is the id chain with a TRAILING delimiter, which is what
    /// makes containment an exact prefix test: `cat_1/` never prefixes
    /// `cat_10/`, so `under` cannot silently include a sibling whose id
    /// merely starts the same way.
    fn path_under(
        &self,
        db: &dyn WriteDb,
        entity: &str,
        parent: Option<&str>,
        id: &str,
    ) -> Result<(String, i64), String> {
        match parent {
            None => Ok((format!("{id}/"), 0)),
            Some(p) => {
                let (pp, pd) = self.node_path(db, entity, p)?.ok_or_else(not_found)?;
                if pd + 1 > TREE_DEPTH_CAP {
                    return Err(format!(
                        "tree_depth_exceeded: `{entity}` is capped at {TREE_DEPTH_CAP} levels"
                    ));
                }
                Ok((format!("{pp}{id}/"), pd + 1))
            }
        }
    }

    /// The path/depth maintenance for a reparent, or `None` when this
    /// statement does not move a tree node.
    ///
    /// Two refusals live here, both settled: a **cycle** (a node cannot
    /// become its own descendant — the check is a prefix test against the
    /// node's own path, which is exactly what "is the new parent inside my
    /// subtree" means), and the **depth cap** applied to the deepest leaf
    /// the move would push down, not merely to the node itself.
    fn reparent_sql(
        &self,
        db: &dyn WriteDb,
        e: &Entity,
        id: &str,
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
    ) -> Result<Option<String>, String> {
        let Some((tref, _)) = self.tree_ref(e) else {
            return Ok(None);
        };
        let Some((_, expr)) = fields.iter().find(|(n, _)| n.name == tref) else {
            return Ok(None);
        };
        let new_parent = match eval_expr(expr, params, caller)?.gate {
            CreateVal::Id(p) | CreateVal::Text(p) => Some(p),
            CreateVal::Null => None,
            other => {
                return Err(format!(
                    "invalid_binding_expr: a tree ref takes an id, got {other:?}"
                ))
            }
        };
        let (old_path, old_depth) = self.node_path(db, &e.name, id)?.ok_or_else(not_found)?;
        if let Some(p) = &new_parent {
            if p == id {
                return Err(format!("tree_cycle: `{id}` cannot be its own parent"));
            }
            let (pp, _) = self.node_path(db, &e.name, p)?.ok_or_else(not_found)?;
            if pp.starts_with(&old_path) {
                return Err(format!("tree_cycle: `{p}` is inside `{id}`'s own subtree"));
            }
        }
        let (new_path, new_depth) = self.path_under(db, &e.name, new_parent.as_deref(), id)?;
        // The cap applies to the whole subtree: moving a three-deep branch
        // one level down pushes its leaves down too.
        let deepest = db.rows(&format!(
            "SELECT max({}) FROM {} WHERE {sp} AND left({}, {}) = {}",
            q_ident("depth"),
            q_ident(&e.name),
            q_ident("$path"),
            old_path.len(),
            sql_str(&old_path),
            sp = self.space_pred(),
        ))?;
        let deepest: i64 = deepest
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|v| v.parse().ok())
            .unwrap_or(old_depth);
        if deepest - old_depth + new_depth > TREE_DEPTH_CAP {
            return Err(format!(
                "tree_depth_exceeded: the move would push `{}` past {TREE_DEPTH_CAP} levels",
                e.name
            ));
        }
        // One prefix substitution over the subtree, this node included.
        Ok(Some(format!(
            "UPDATE {t} SET {path} = {new} || substring({path} from {cut}),              {depth} = {depth} + ({shift}) WHERE {sp} AND left({path}, {plen}) = {old};",
            t = q_ident(&e.name),
            sp = self.space_pred(),
            path = q_ident("$path"),
            depth = q_ident("depth"),
            new = sql_str(&new_path),
            cut = old_path.len() + 1,
            shift = new_depth - old_depth,
            plen = old_path.len(),
            old = sql_str(&old_path),
        )))
    }

    /// **Owned rows die with their owner** (owned-delete, 2026-09-29): every
    /// row that references a doomed row through an `owned` ref goes with
    /// it, transitively, and that includes the access rows Ordo derives for
    /// an entity with an `acl` (their refs are `owned`). Returns the
    /// doomed rows, roots included, each row once.
    ///
    /// A tree's own parent ref is left to `refuse_tree_parent` and to
    /// `subtree`: a plain `del` of a tree node with children still
    /// refuses, `tree owned` or not.
    fn owned_cascade(
        &self,
        db: &dyn WriteDb,
        roots: &[(String, String)],
    ) -> Result<Vec<(String, String)>, String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (entity, id) in roots {
            self.visit_owned(db, entity, id, &mut seen, &mut out)?;
        }
        Ok(out)
    }

    fn visit_owned(
        &self,
        db: &dyn WriteDb,
        entity: &str,
        id: &str,
        seen: &mut std::collections::HashSet<(String, String)>,
        out: &mut Vec<(String, String)>,
    ) -> Result<(), String> {
        if !seen.insert((entity.to_string(), id.to_string())) {
            return Ok(());
        }
        for other in &self.schema.entities {
            for f in &other.fields {
                let FieldTy::Ref {
                    target,
                    owned: true,
                    tree,
                } = &f.ty
                else {
                    continue;
                };
                if target != entity || (*tree && other.name == entity) {
                    continue;
                }
                let kids = first_col(db.rows(&format!(
                    "SELECT {} FROM {} WHERE {sp} AND {} = {} ORDER BY {}",
                    q_ident("id"),
                    q_ident(&other.name),
                    q_ident(&f.name),
                    sql_str(id),
                    q_ident("id"),
                    sp = self.space_pred(),
                ))?);
                for k in kids {
                    self.visit_owned(db, &other.name, &k, seen, out)?;
                }
            }
        }
        out.push((entity.to_string(), id.to_string()));
        Ok(())
    }

    /// Lock the cascaded rows the statement has not locked already.
    fn lock_cascaded(
        &self,
        db: &dyn WriteDb,
        victims: &[(String, String)],
        roots: &[(String, String)],
    ) -> Result<(), String> {
        for (entity, id) in victims {
            if !roots.iter().any(|r| r.0 == *entity && r.1 == *id) {
                self.lock_row(db, entity, id)?;
            }
        }
        Ok(())
    }

    /// The caller must be allowed to delete every row the cascade takes,
    /// not the roots alone, and the cascade is all or nothing. The spec
    /// says so for `subtree` ("guards are evaluated over the whole
    /// subtree") and is silent for owned rows, so the same rule is applied:
    /// a partial cascade would leave a row without its owner. The refusal
    /// counts the rows and never names them.
    fn refuse_unwritable_cascade(
        &self,
        db: &dyn WriteDb,
        victims: &[(String, String)],
        roots: &[(String, String)],
        caller: &str,
    ) -> Result<(), String> {
        let mut blocked = 0;
        for (entity, id) in victims {
            if roots.iter().any(|r| r.0 == *entity && r.1 == *id) {
                continue;
            }
            if !self.row_visible(db, entity, id, caller, true)? {
                blocked += 1;
            }
        }
        if blocked > 0 {
            return Err(format!(
                "access_guard: the rows owned by what you are deleting include {blocked} row{} \
                 you may not delete — an owner does not go without what it owns, so none of it \
                 is deleted",
                if blocked == 1 { "" } else { "s" }
            ));
        }
        Ok(())
    }

    /// **Non-tree refs restrict deletes** (§write rules 6, settled):
    /// deleting a row other rows point at through a plain reference is
    /// refused, naming a referrer. A referrer that is itself going in the
    /// same cascade does not count. Owned referrers always are, so they
    /// are not looked for here.
    ///
    /// This also covers a row the cascade takes through an owned ref that
    /// something outside the cascade still references: the whole delete
    /// refuses, because a foreign key would otherwise raise two rows into
    /// it.
    ///
    /// Without this the database's foreign key raises instead — a raw
    /// constraint name and a table name, leaking physical schema through
    /// an error the closed inventory never promised. The platform owes the
    /// caller its own vocabulary here, and owes it BEFORE the statement
    /// runs rather than as a rollback afterwards.
    fn refuse_plain_referrers(
        &self,
        db: &dyn WriteDb,
        victims: &[(String, String)],
    ) -> Result<(), String> {
        for (entity, id) in victims {
            for other in &self.schema.entities {
                for f in &other.fields {
                    let FieldTy::Ref {
                        target,
                        owned,
                        tree,
                    } = &f.ty
                    else {
                        continue;
                    };
                    // A tree's own parent ref is the children check's
                    // business, and it phrases the refusal better.
                    if target != entity || *owned || (*tree && other.name == *entity) {
                        continue;
                    }
                    let going: Vec<String> = victims
                        .iter()
                        .filter(|v| v.0 == other.name)
                        .map(|v| sql_str(&v.1))
                        .collect();
                    let excluded = if going.is_empty() {
                        String::new()
                    } else {
                        format!(" AND {} NOT IN ({})", q_ident("id"), going.join(", "))
                    };
                    let rows = db.rows(&format!(
                        "SELECT {} FROM {} WHERE {sp} AND {} = {}{excluded} LIMIT 1",
                        q_ident("id"),
                        q_ident(&other.name),
                        q_ident(&f.name),
                        sql_str(id),
                        sp = self.space_pred(),
                    ))?;
                    if let Some(r) = rows.first().and_then(|r| r.first().cloned().flatten()) {
                        return Err(format!(
                            "has_referrers: `{id}` is referenced by `{r}` ({}.{}) — remove or \
                             repoint the referrers first",
                            other.name, f.name
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Put the doomed rows in an order a foreign key accepts: a row goes
    /// only after every doomed row that references it, by any ref. The
    /// foreign keys are not deferrable, so a parent deleted before its
    /// child is a violation. Ties keep the order the rows came in.
    fn delete_order(
        &self,
        db: &dyn WriteDb,
        victims: Vec<(String, String)>,
    ) -> Result<Vec<(String, String)>, String> {
        if victims.len() < 2 {
            return Ok(victims);
        }
        let index: std::collections::HashMap<(String, String), usize> = victims
            .iter()
            .enumerate()
            .map(|(i, v)| (v.clone(), i))
            .collect();
        // waits[j] = how many doomed rows must go before row j;
        // after[i] = the rows that wait on row i.
        let mut waits = vec![0usize; victims.len()];
        let mut after: Vec<Vec<usize>> = vec![Vec::new(); victims.len()];
        for (j, (entity, id)) in victims.iter().enumerate() {
            for other in &self.schema.entities {
                for f in &other.fields {
                    let FieldTy::Ref { target, .. } = &f.ty else {
                        continue;
                    };
                    if target != entity {
                        continue;
                    }
                    let ids: Vec<String> = victims
                        .iter()
                        .filter(|v| v.0 == other.name)
                        .map(|v| sql_str(&v.1))
                        .collect();
                    if ids.is_empty() {
                        continue;
                    }
                    let referrers = first_col(db.rows(&format!(
                        "SELECT {} FROM {} WHERE {sp} AND {} = {} AND {} IN ({})",
                        q_ident("id"),
                        q_ident(&other.name),
                        q_ident(&f.name),
                        sql_str(id),
                        q_ident("id"),
                        ids.join(", "),
                        sp = self.space_pred(),
                    ))?);
                    for r in referrers {
                        if let Some(&i) = index.get(&(other.name.clone(), r)) {
                            if i != j {
                                waits[j] += 1;
                                after[i].push(j);
                            }
                        }
                    }
                }
            }
        }
        let mut ready: std::collections::BinaryHeap<std::cmp::Reverse<usize>> = waits
            .iter()
            .enumerate()
            .filter(|(_, w)| **w == 0)
            .map(|(i, _)| std::cmp::Reverse(i))
            .collect();
        let mut out = Vec::with_capacity(victims.len());
        while let Some(std::cmp::Reverse(i)) = ready.pop() {
            out.push(victims[i].clone());
            for &j in &after[i] {
                waits[j] -= 1;
                if waits[j] == 0 {
                    ready.push(std::cmp::Reverse(j));
                }
            }
        }
        if out.len() != victims.len() {
            return Err(
                "has_referrers: the rows this delete takes reference each other in a \
                 cycle — remove or repoint the referrers first"
                    .to_string(),
            );
        }
        Ok(out)
    }

    /// One `Delete` command with a before-image and one `DELETE` per row,
    /// in the order given. Per-row commands are what make the cascade
    /// revertible, and the image is built the same way a direct delete
    /// builds it, so a sealed field is bound to its own row.
    #[allow(clippy::type_complexity)]
    fn cascade_delete_plan(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        victims: &[(String, String)],
    ) -> Result<(Vec<PendingCommand>, Vec<String>), String> {
        let mut commands = Vec::new();
        let mut sql = Vec::new();
        for (entity, id) in victims {
            let before = self
                .row_image(db, log, entity, id)?
                .ok_or_else(|| format!("row `{id}` vanished"))?;
            commands.push(PendingCommand {
                entity_name: entity.clone(),
                row_id: id.clone(),
                kind: CommandKind::Delete,
                before: Some(before),
                after: None,
            });
            sql.push(format!(
                "DELETE FROM {} WHERE {sp} AND {} = {};",
                q_ident(entity),
                q_ident("id"),
                sql_str(id),
                sp = self.space_pred(),
            ));
        }
        Ok((commands, sql))
    }

    /// Deleting a tree node with children is refused, and the message
    /// states the count (§tree, settled). Subtree deletion is a separate,
    /// greppable spelling — deliberately not something a plain `del`
    /// slides into.
    fn refuse_tree_parent(&self, db: &dyn WriteDb, e: &Entity, id: &str) -> Result<(), String> {
        let Some((tref, _)) = self.tree_ref(e) else {
            return Ok(());
        };
        let rows = db.rows(&format!(
            "SELECT count(*) FROM {} WHERE {sp} AND {} = {}",
            q_ident(&e.name),
            q_ident(&tref),
            sql_str(id),
            sp = self.space_pred(),
        ))?;
        let n: i64 = rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if n > 0 {
            return Err(format!(
                "tree_has_children: `{id}` has {n} child{} — reparent them, or spell the                  subtree delete explicitly",
                if n == 1 { "" } else { "ren" }
            ));
        }
        Ok(())
    }

    /// **Rule 8's runtime clause.** With `id` in the braces the id
    /// addresses and every other unique field is an ordinary value — but a
    /// value that already identifies ANOTHER row makes the statement two
    /// identity claims pointing at two rows. Rejected as
    /// `conflicting_identity`, nothing applied.
    ///
    /// The message names the KEY, never the row it collided with: the
    /// caller supplied that value, so "this key is taken" is what they
    /// need and is no more than the unique index itself would leak.
    ///
    /// Without this the index raises instead and the caller gets
    /// `duplicate` — the class for a constraint they did not anticipate —
    /// when what actually happened is that their write named two rows.
    #[allow(clippy::too_many_arguments)]
    fn check_conflicting_identity(
        &self,
        db: &dyn WriteDb,
        e: &Entity,
        id: &str,
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
    ) -> Result<(), String> {
        for (name, expr) in fields {
            let Some(f) = e.field(&name.name) else {
                continue;
            };
            if !f.unique || f.implicit {
                continue;
            }
            let v = eval_expr(expr, params, caller)?;
            if matches!(v.gate, CreateVal::Null) {
                continue;
            }
            let (column, literal) = self.key_address(e, f, &v)?;
            let rows = db.rows(&format!(
                "SELECT {id_c} FROM {} WHERE {sp} AND {} = {literal} AND {id_c} <> {} LIMIT 1",
                q_ident(&e.name),
                q_ident(&column),
                sql_str(id),
                sp = self.space_pred(),
                id_c = q_ident("id"),
            ))?;
            if !rows.is_empty() {
                return Err(format!(
                    "conflicting_identity: this write addresses `{id}` ({}) but `{}` already \
                     identifies another row — one write, two identities, nothing applied",
                    e.name, f.name
                ));
            }
        }
        // **Composite keys** (`unique (board, member)` — the shape `acl`
        // derives) need the same test, and it has to run over the row as
        // it WILL BE: a statement that assigns one column of a two-column
        // key still moves the row onto a new tuple, and if another row
        // holds that tuple the write is refused. The columns the statement
        // does not assign are read back from the addressed row inside the
        // same query, so there is no window between reading them and
        // testing them.
        //
        // Two answers, because they are two different mistakes:
        //
        // - the braces cover the **whole** key → the caller stated a
        //   second identity on purpose, and that is rule 8's
        //   `conflicting_identity`;
        // - the braces cover **part** of it → the caller changed a field
        //   and the resulting row collides. They did not claim a second
        //   identity; the data refused. That is `duplicate`, the class for
        //   a constraint the caller did not anticipate.
        //
        // Either way the point is that this is answered HERE. Without it
        // the unique index raises, and a raw Postgres constraint violation
        // reaches the caller as a 500 in a vocabulary the platform does
        // not speak.
        for key in &e.unique_keys {
            let mut eqs = Vec::with_capacity(key.len());
            let mut assigned = 0usize;
            let mut skip = false;
            for name in key {
                let Some(f) = e.field(name) else {
                    skip = true;
                    break;
                };
                match fields.iter().find(|(n, _)| &n.name == name) {
                    Some((_, expr)) => {
                        let v = eval_expr(expr, params, caller)?;
                        // A null component addresses no tuple, and
                        // Postgres's `NULLS DISTINCT` agrees: nothing
                        // collides with it.
                        if matches!(v.gate, CreateVal::Null) {
                            skip = true;
                            break;
                        }
                        let (column, literal) = self.key_address(e, f, &v)?;
                        eqs.push(format!("{} = {literal}", q_ident(&column)));
                        assigned += 1;
                    }
                    // Unassigned: whatever the addressed row already holds.
                    None => eqs.push(format!(
                        "{c} = (SELECT {c} FROM {t} WHERE {sp} AND {i} = {lit})",
                        c = q_ident(&f.column()),
                        t = q_ident(&e.name),
                        i = q_ident("id"),
                        lit = sql_str(id),
                        sp = self.space_pred(),
                    )),
                }
            }
            // Nothing this statement writes touches the key, so it cannot
            // move the row onto another row's tuple.
            if skip || assigned == 0 {
                continue;
            }
            let rows = db.rows(&format!(
                "SELECT {id_c} FROM {} WHERE {sp} AND {} AND {id_c} <> {} LIMIT 1",
                q_ident(&e.name),
                eqs.join(" AND "),
                sql_str(id),
                sp = self.space_pred(),
                id_c = q_ident("id"),
            ))?;
            if rows.is_empty() {
                continue;
            }
            let named = key.join(", ");
            return Err(if assigned == key.len() {
                format!(
                    "conflicting_identity: this write addresses `{id}` ({}) but `{named}` \
                     already identifies another row — one write, two identities, nothing \
                     applied",
                    e.name
                )
            } else {
                format!(
                    "duplicate: `{}.({named})` is already taken by another row, so this \
                     write would make two — nothing applied",
                    e.name
                )
            });
        }
        Ok(())
    }

    /// Where a unique key lives in SQL, and the literal that addresses it.
    ///
    /// A **sealed** key addresses through its blind index, not its
    /// payload: the payload is an AEAD envelope with a fresh nonce, so two
    /// seals of one value never compare equal, and the digest is both the
    /// only comparable spelling and exactly what the unique index is built
    /// over. `normalize(lower)` runs first, so `Zed@Example.COM` and
    /// `zed@example.com` address the same row — the same normalization the
    /// read side digests with.
    fn key_address(
        &self,
        e: &Entity,
        f: &ordo_schema::Field,
        v: &EvaledVal,
    ) -> Result<(String, String), String> {
        if !f.sealed {
            return Ok((f.column(), v.sql.clone()));
        }
        if !f.indexed {
            // Uniqueness over an unindexed sealed field has nothing to
            // compare against; the schema cannot even build the index.
            return Err(format!(
                "invalid_binding_expr: sealed `{}.{}` has no blind index, so it cannot address \
                 a row",
                e.name, f.name
            ));
        }
        let plain = match &v.gate {
            CreateVal::Text(t) | CreateVal::Id(t) => t.clone(),
            // Deliberately NOT the null sentinel, and no longer `= NULL`.
            // The sentinel makes null a comparable digest, so addressing
            // by it would now *work* — and DL (uniqueness) rules that a
            // converge-or-create whose addressing key is null is a typed
            // rejection, never a create and never a converge. The old
            // `= NULL` literal reached the same place by accident and by
            // the worse route: it matched nothing and created a duplicate.
            CreateVal::Null => {
                return Err(format!(
                    "null_key_address: `{}.{}` is null, which addresses no row",
                    e.name, f.name
                ))
            }
            other => {
                return Err(format!(
                    "invalid_binding_expr: sealed `{}` takes text, got {other:?}",
                    f.name
                ))
            }
        };
        let normalized = match f.normalize.as_deref() {
            Some("lower") => plain.to_lowercase(),
            _ => plain,
        };
        let digest = self
            .keys
            .blind_index(
                self.space(),
                &e.name,
                &f.name,
                ordo_crypto::BlindValue::Text(&normalized),
            )
            .map_err(|er| format!("blind index: {er}"))?;
        Ok((
            f.index_column(),
            format!("'\\x{}'", ordo_crypto::hex(&digest)),
        ))
    }

    /// The row a unique key names, if any. `lock` takes `FOR UPDATE` on
    /// it, which is what makes the converge-or-create branch decision hold
    /// still for the rest of the transaction.
    fn row_by_key(
        &self,
        db: &dyn WriteDb,
        entity: &str,
        columns: &[String],
        literals: &[String],
        lock: bool,
    ) -> Result<Option<String>, String> {
        let eqs: Vec<String> = columns
            .iter()
            .zip(literals)
            .map(|(c, l)| format!("{} = {l}", q_ident(c)))
            .collect();
        let rows = db.rows(&format!(
            "SELECT {} FROM {} WHERE {sp} AND {}{}",
            q_ident("id"),
            q_ident(entity),
            eqs.join(" AND "),
            if lock { " FOR UPDATE" } else { "" },
            sp = self.space_pred(),
        ))?;
        Ok(rows.first().and_then(|r| r.first().cloned().flatten()))
    }

    /// The row a condition addresses when that condition **is** one of the
    /// entity's unique keys — every column of the key compared for
    /// equality against a literal or a param, and nothing else.
    ///
    /// `Ok(None)` means "not a key" and the caller should range. A
    /// condition that IS a key but names no row is `not_found`, exactly as
    /// an id that names no row is: it addressed something, and the
    /// something is not there.
    ///
    /// Two guards on the shape, both load-bearing:
    ///
    /// - **Every** column of the key must be present. A proper subset of a
    ///   composite key selects a SET, and treating it as an address would
    ///   silently delete one of several matching rows.
    /// - Nothing else may be present. `board == $b and member == $m and
    ///   level == "read"` is a filtered delete: the extra conjunct can
    ///   make it match nothing, so it is not an identity claim.
    fn addressed_by_key(
        &self,
        db: &dyn WriteDb,
        entity: &str,
        cond: &Condition,
        params: &[(String, Expr)],
        caller: &str,
    ) -> Result<Option<String>, String> {
        let Some(eqs) = flat_equalities(cond) else {
            return Ok(None);
        };
        let Some(e) = self.schema.entity(entity) else {
            return Ok(None);
        };
        let named: Vec<&str> = eqs.iter().map(|(n, _)| n.as_str()).collect();
        // Single-column unique fields and composite keys, one rule.
        let key: Vec<String> = e
            .unique_keys
            .iter()
            .cloned()
            .chain(
                e.fields
                    .iter()
                    .filter(|f| f.unique && !f.implicit)
                    .map(|f| vec![f.name.clone()]),
            )
            .find(|k| k.len() == named.len() && k.iter().all(|c| named.contains(&c.as_str())))
            .unwrap_or_default();
        if key.is_empty() {
            return Ok(None);
        }
        let mut columns = Vec::with_capacity(key.len());
        let mut literals = Vec::with_capacity(key.len());
        let mut shown = Vec::with_capacity(key.len());
        for name in &key {
            let f = e.field(name).expect("named from the schema");
            let expr = eqs
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, x)| *x)
                .expect("matched above");
            let v = eval_expr(expr, params, caller)?;
            match &v.gate {
                CreateVal::Null => {
                    return Err(format!(
                        "null_key_address: `{entity}.{name}` is null, which addresses no row"
                    ))
                }
                // Echoed in the refusal below, and only ever the caller's
                // own input — a sealed key shows its plaintext argument,
                // never the stored envelope.
                CreateVal::Id(t) | CreateVal::Text(t) => shown.push(t.clone()),
                other => shown.push(format!("{other:?}")),
            }
            let (column, literal) = self.key_address(e, f, &v)?;
            columns.push(column);
            literals.push(literal);
        }
        match self.row_by_key(db, entity, &columns, &literals, false)? {
            Some(id) => Ok(Some(id)),
            // The same shape the id-addressed form answers with, for the
            // same reason: an address that names no row is `not_found`,
            // and the message echoes only what the caller supplied.
            None => Err(format!("not_found: `{}` ({entity})", shown.join(", "))),
        }
    }

    /// Is the caller a **member of this space**, as bootstrap rule 4
    /// means it?
    ///
    /// One question, one column: a `members` row in this space whose
    /// `role` is not null. The spec defines a guest as exactly that —
    /// "bound to the space through an inter-space share, holding a members
    /// row with a null `role`" — and a principal with no members row at
    /// all (a routine, the system, a source) is the other half of the same
    /// answer, which is why absence and a null role both come back false
    /// from one query rather than from two rules that could drift.
    ///
    /// The space predicate is not decoration: `members` is per-space, so
    /// "is this principal a member" is only a question about THIS space,
    /// and a caller who is a member of a neighbouring one is a guest here.
    pub(crate) fn caller_is_space_member(
        &self,
        db: &dyn WriteDb,
        caller: &str,
    ) -> Result<bool, String> {
        if caller.is_empty() {
            return Ok(false);
        }
        let rows = db.rows(&format!(
            "SELECT {r} FROM {} WHERE {sp} AND {} = {} AND {r} IS NOT NULL",
            q_ident("members"),
            q_ident("id"),
            sql_str(caller),
            r = q_ident("role"),
            sp = self.space_pred(),
        ))?;
        Ok(!rows.is_empty())
    }

    /// `SELECT … FOR UPDATE` on one row. Outside a transaction this is a
    /// no-op lock released immediately — which is exactly why the psql
    /// path cannot claim the atomicity the transactional path has.
    fn lock_row(&self, db: &dyn WriteDb, entity: &str, id: &str) -> Result<(), String> {
        db.rows(&format!(
            "SELECT {} FROM {} WHERE {sp} AND {} = {} FOR UPDATE",
            q_ident("id"),
            q_ident(entity),
            q_ident("id"),
            sql_str(id),
            sp = self.space_pred(),
        ))
        .map(|_| ())
    }

    /// Is `id` in the caller's read (or write) view? Unguarded entities
    /// answer with bare existence.
    fn row_visible(
        &self,
        db: &dyn WriteDb,
        entity: &str,
        id: &str,
        caller: &str,
        write_verb: bool,
    ) -> Result<bool, String> {
        let sql = match guard_filter_query(&self.schema, entity, write_verb, Some(caller))
            .map_err(|er| format!("bind: {er}"))?
        {
            Some(q) => format!(
                "SELECT 1 FROM ({}) v WHERE v.{} = {}",
                plan(&q, &self.plan_space())
                    .map_err(|er| format!("plan: {er}"))?
                    .sql
                    .trim_end_matches(';'),
                q_ident("id"),
                sql_str(id)
            ),
            None => format!(
                "SELECT 1 FROM {} WHERE {sp} AND {} = {}",
                q_ident(entity),
                q_ident("id"),
                sql_str(id),
                sp = self.space_pred(),
            ),
        };
        Ok(!db.rows(&sql)?.is_empty())
    }

    /// Record a sealed value in the payload store and return the handle
    /// the image carries in its place — **the** sealed-image write.
    ///
    /// Everything §7 pins about a `SealedRef` happens here and only here:
    /// the handle is minted at random by the key provider, salted per space
    /// and per epoch in its derivation; the epoch comes off the envelope
    /// the value was sealed under, so a payload written before a rotation
    /// keeps naming the key that opens it; and the payload is staged for
    /// storage **as a new row every time**, with no lookup, no upsert and
    /// no comparison against anything already stored. Two images of one
    /// value are two handles over two rows, which is what stops the log
    /// from becoming an equality index over sealed fields.
    fn seal_image_ref(
        &self,
        log: &mut WriteLog,
        entity: &str,
        field: &str,
        anchor: &str,
        sealed: &[u8],
    ) -> Result<LogValue, String> {
        let epoch = ordo_crypto::envelope_epoch(sealed)
            .ok_or_else(|| format!("internal: `{entity}.{field}` holds no sealed envelope"))?;
        let lease = self
            .keys
            .release(self.space(), ordo_crypto::Purpose::Write, "image")
            .map_err(|e| format!("key release: {e}"))?;
        let handle = self
            .keys
            .mint_sealed_ref(&lease, epoch)
            .map_err(|e| format!("sealed ref: {e}"))?;
        log.payloads.push(PendingPayload {
            handle,
            epoch,
            entity: entity.to_string(),
            field: field.to_string(),
            anchor: anchor.to_string(),
            bytes: sealed.to_vec(),
        });
        Ok(LogValue::SealedRef(handle))
    }

    /// `$blobref.integrity` for every non-null external blob column of
    /// `row`, keyed by field name: the one extra read
    /// `docs/plan-blob-chain.md` §4.1 asks for per image, run here because
    /// `ordo_schema::image` has no I/O and `row_image` is the caller it
    /// names.
    ///
    /// **The only source, by design.** `PlatformValue` briefly carried a
    /// `digest_envelope` method for the upload's own mint-time copy of the
    /// envelope (C4), but that copy only ever exists on the upload path: a
    /// delete's before-image and a revert's images have no `PlatformValue`
    /// to ask, and they need this exact same read. Two sources would mean
    /// two answers to "what is this column's digest", differing exactly
    /// when a bug made them differ, so `digest_envelope` is gone from the
    /// trait (C5) and every image, upload's included, is answered by this
    /// lookup and none other.
    ///
    /// A non-null external blob column with no matching `$blobref` row is
    /// refused rather than silently imaged without a digest: the only
    /// writer of such a column is `run_platform_converge`, which inserts
    /// the reference before the column ever names it (§2.2), so a live
    /// column with nothing behind it is corruption, not the "image
    /// written before C5" case, which has no column value to fail
    /// this lookup with, because it predates this function running at
    /// all.
    fn blob_digests(
        &self,
        db: &dyn WriteDb,
        e: &Entity,
        row: &[Option<String>],
    ) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let mut out = BTreeMap::new();
        for (f, cell) in ordo_schema::image::image_columns(e).iter().zip(row) {
            if !ordo_schema::image::is_external_blob(f) {
                continue;
            }
            let Some(raw) = cell else { continue };
            let handle = ordo_crypto::blobref::parse_handle(raw).ok_or_else(|| {
                format!(
                    "internal: `{}.{}` does not hold a well-formed blob handle",
                    e.name, f.name
                )
            })?;
            let rows = db.rows(&ordo_crypto::blobref::lookup_sql(self.space(), &handle))?;
            let bref = rows
                .first()
                .and_then(|r| ordo_crypto::blobref::blob_ref_of(r))
                .ok_or_else(|| {
                    format!(
                        "internal: `{}.{}` names a blob handle with no `$blobref` row",
                        e.name, f.name
                    )
                })?;
            out.insert(f.name.clone(), bref.integrity);
        }
        Ok(out)
    }

    /// The full field image of one row, as the log records it.
    ///
    /// The decoding rule (which column, which scalar type, a sealed
    /// field's envelope-or-null branch) is `ordo_schema::image`'s, shared
    /// with `undo::row_image` (`docs/plan-blob-chain.md` §3, C2). What
    /// stays here is the I/O: the select runs over this write's own
    /// `WriteDb` (`psql` in the harness, a transaction on the executor
    /// path), and a sealed field's envelope is staged into `log.payloads`
    /// rather than written immediately, flushed once the enclosing
    /// group has a correlation (`:728`), which is why an image built here
    /// can still be discarded for free if the write it belongs to never
    /// commits. The blob digest read (`blob_digests`, above) is not
    /// staged: it is a read, not a write, so there is nothing to defer.
    pub(crate) fn row_image(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        entity: &str,
        id: &str,
    ) -> Result<Option<Image>, String> {
        let e = self.schema.entity(entity).ok_or("unknown entity")?;
        let rows = db.rows(&ordo_schema::image::image_select(e, id, &self.space().0))?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let blob_digests = self.blob_digests(db, e, &row)?;
        let mut sink = ConformanceSealSink { fixture: self, log };
        ordo_schema::image::image_of_row(e, &row, &blob_digests, &mut sink)
            .map(Some)
            .map_err(|err| err.to_string())
    }

    /// Every ref this assignment list re-points, with its new value. A
    /// reassignment is the create case in disguise: you are naming a
    /// parent, so the parent must be one you can see (G12) or the answer
    /// is `not_found`, exactly as it would be on a create.
    fn reassigned_refs(
        &self,
        e: &Entity,
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
    ) -> Result<Vec<(String, String, bool)>, String> {
        let mut out = Vec::new();
        for (name, expr) in fields {
            let Some(f) = e.field(&name.name) else {
                continue;
            };
            let FieldTy::Ref { target, owned, .. } = &f.ty else {
                continue;
            };
            // Classified by the DECLARED type, not by how the value is
            // spelled: this field is a ref, so whatever arrives is an id.
            if let CreateVal::Id(v) | CreateVal::Text(v) = eval_expr(expr, params, caller)?.gate {
                out.push((target.clone(), v, *owned));
            }
        }
        Ok(out)
    }

    /// `rev` in a write's braces is a **belief about the row**, not a
    /// field assignment — the platform owns `rev` and always has. It is
    /// split out here before anything else looks at the braces (§6 rule 9).
    ///
    /// Two beliefs, one spelling:
    ///
    /// - `rev: $r` — "the row is at version r". Converge if so, `cas_miss`
    ///   otherwise. This is what makes a read-modify-write safe without a
    ///   lock held across the caller's thinking time: the caller states
    ///   the version they reasoned about, and a write that raced them is
    ///   refused instead of silently overwriting.
    /// - `rev: null` — "there is no such row", i.e. create-once. Paired
    ///   with `id ==` it is incoherent and rejected at compile time: an id
    ///   is only obtainable from a row that existed, so asserting its
    ///   absence contradicts itself.
    fn split_rev(
        &self,
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
        addressed: bool,
    ) -> Result<(RevBelief, Vec<(ordo_syntax::Ident, Expr)>), String> {
        let mut belief = None;
        let mut rest = Vec::new();
        for (name, expr) in fields {
            if name.name != "rev" {
                rest.push((name.clone(), expr.clone()));
                continue;
            }
            let v = eval_expr(expr, params, caller)?;
            belief = Some(match v.gate {
                CreateVal::Null => {
                    if addressed {
                        return Err(
                            "invalid_binding_expr: `id` with `rev: null` asserts that a row \
                             you already have the id of does not exist"
                                .into(),
                        );
                    }
                    None
                }
                CreateVal::Int(n) => Some(n),
                other => {
                    return Err(format!(
                        "param_type_mismatch: `rev` is a version number, got {other:?}"
                    ))
                }
            });
        }
        Ok((belief, rest))
    }

    /// Check a stated `rev` against the row's actual version.
    fn check_cas(&self, db: &dyn WriteDb, entity: &str, id: &str, want: i64) -> Result<(), String> {
        let rows = db.rows(&format!(
            "SELECT {} FROM {} WHERE {sp} AND {} = {}",
            q_ident("rev"),
            q_ident(entity),
            q_ident("id"),
            sql_str(id),
            sp = self.space_pred(),
        ))?;
        let actual: i64 = rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|v| v.parse().ok())
            .ok_or_else(not_found)?;
        if actual != want {
            // The message states both versions: a caller retrying needs to
            // know whether they are one write behind or many.
            return Err(format!(
                "cas_miss: `{id}` ({entity}) is at rev {actual}, the write expected {want}"
            ));
        }
        Ok(())
    }

    /// Re-pointing a ref is a claim about the row on the other end.
    ///
    /// For a plain reference (`assignee -> members?`) it is enough that the
    /// target be READABLE: you are naming someone, not modifying them.
    ///
    /// For an **owned** ref it is not. Moving a child into a parent changes
    /// what that parent contains, so the caller must be able to WRITE the
    /// new parent. Requiring only readability here is an escalation with
    /// two shapes, both proven against a live database before this check
    /// existed: a project member re-pointed into someone else's project
    /// carries its member with it, and — worse, because it is
    /// self-satisfying — an acl access row re-pointed onto another board
    /// makes its holder an admin there, after which the post-image guard
    /// happily passes *because of the row that was just moved*. Proving
    /// the guard on the TARGET, before the move, is the check that cannot
    /// be satisfied by its own effect.
    fn check_reassignment(
        &self,
        db: &dyn WriteDb,
        e: &Entity,
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
    ) -> Result<(), String> {
        for (target, v, owned) in self.reassigned_refs(e, fields, params, caller)? {
            if !self.row_visible(db, &target, &v, caller, false)? {
                return Err(NOT_FOUND.to_string());
            }
            if owned && !self.row_visible(db, &target, &v, caller, true)? {
                return Err(format!(
                    "access_guard: `{v}` ({target}) is not yours to add to"
                ));
            }
        }
        Ok(())
    }

    /// **What kind of right-hand side is this?** (17f, DL §6 "RHS field
    /// references"; DL §3 "Legal anywhere expressions are: … write RHS".)
    ///
    /// One classifier, consulted by the fold and by the assignment builder,
    /// so a value cannot be one kind on the way in and another on the way
    /// out. Every refusal in this function is by NAME with its reason —
    /// none of them is a limitation of the renderer, and each says what the
    /// author should write instead.
    fn classify_rhs(
        &self,
        e: &Entity,
        f: &ordo_schema::Field,
        expr: &Expr,
        params: &[(String, Expr)],
        caller: &str,
        label: &str,
    ) -> Result<Rhs, String> {
        if is_held_value(expr, params, f.ty.value_ty()) {
            return Ok(Rhs::Held);
        }
        // The alias is the table's own name, so the rendered fragment reads
        // `"teams"."used"` and needs no FROM — which is exactly what an
        // `UPDATE "teams" SET …` can evaluate.
        let bound = ordo_sema::write_value_expr(
            &self.schema,
            &e.name,
            &e.name,
            expr,
            params,
            Some(caller),
            f.ty.value_ty(),
        )
        .map_err(|er| format!("{}: {}", er.code, er.message))?;
        let sql = ordo_plan::render_write_value(&bound.expr, &self.plan_space())
            .map_err(|er| format!("{}: {}", er.code, er.message))?;
        if !bound.reads_row {
            return Ok(Rhs::RowFree(sql));
        }

        // ── from here: the value only exists once the row is in hand ────

        // **A create has no current row.** DL §6 spells the computed form
        // as the increment `{ n: n + 1 }` — "computed server-side inside the
        // ROW's transaction" — and every mode whose label can be `creates`
        // has a branch on which there is no such row. Converge-or-create is
        // the sharp case: `INSERT … ON CONFLICT DO UPDATE` decides the
        // branch in the database, so admitting a field reference would mean
        // the same statement reading a row on one branch and inventing a
        // value on the other. The owner ruled the other branch (2026-10-31):
        // a converge-or-create reads each field as its declared default
        // when it creates, so `n: n + 1` stores 1 first and `n + 1` after.
        // [`Fixture::plan_insert`] computes that create value; a field
        // with no default is refused here, as at registration.
        if label == "creates-or-converges" {
            if let Err((read, _)) = ordo_plan::writes::create_branch_reads(e, expr) {
                return Err(format!(
                    "invalid_binding_expr: {}",
                    ordo_plan::writes::no_default_on_create(&e.name, &f.name, &read)
                ));
            }
        } else if !matches!(label, "converges" | "bulk") {
            return Err(format!(
                "invalid_binding_expr: `{}.{}` reads the row this statement writes, and this \
                 statement `{label}` — a create has no current row to read. Address the row \
                 (`set {}(id == …) {{ … }}` or `{{ id: …, … }}`), or write the value out",
                e.name, f.name, e.name,
            ));
        }
        // The rest are fields whose value the platform must HOLD before the
        // write, for a reason that is not about arithmetic. Each of these is
        // a real check the write protocol runs host-side against the value,
        // and a value the database computes arrives too late for all of
        // them.
        let refusal = |why: &str| {
            Err(format!(
                "invalid_binding_expr: `{}.{}` {why} — a computed right-hand side is evaluated \
                 by the statement itself, so the platform never holds the value. Compute it \
                 from parameters instead, or read the row and pass the value in",
                e.name, f.name
            ))
        };
        if f.sealed {
            return refusal("is sealed, and sealing needs the plaintext");
        }
        if (f.unique && !f.implicit) || e.unique_keys.iter().any(|k| k.contains(&f.name)) {
            return refusal(
                "carries a unique key, and rule 8 must know which row the value identifies",
            );
        }
        if matches!(f.ty, FieldTy::Ref { .. }) {
            return refusal("is a ref, and G12 must prove the target is one you can see");
        }
        if f.enum_values.is_some() {
            return refusal(
                "is pinned to a closed set of values, which is checked before the write",
            );
        }
        if matches!(f.ty.value_ty(), ScalarTy::Blob | ScalarTy::Uri) {
            return refusal(
                "is content: a blob handle is minted by an upload and a uri is validated at \
                 the door, so neither is a thing an expression computes",
            );
        }
        Ok(Rhs::RowReading(sql))
    }

    /// **Money and decimal literals, in the units the column stores.**
    ///
    /// Ruled 2026-10-04 (27ak): a `money(p)` value is written in major
    /// units and stored in minor units. A field whose right-hand side is a
    /// numeric literal, or a parameter bound to one, gets the stored value
    /// in its place: `price: 29.99` on a `money(2)` becomes `price: 2999`,
    /// and `rate: 0.08` on a `decimal(4)` stays as written once it is known
    /// to fit. A literal finer than the places is `implicit_rounding`.
    ///
    /// The rewrite happens once and before everything else that reads a
    /// value, for the reason [`substitute_bound`] gives: downstream code
    /// sees the ordinary closed set, and nothing can convert a value twice.
    /// Computed right-hand sides are left alone; the binder converts those
    /// (`ordo_sema::write_value_expr`).
    fn fit_scaled_literals(
        &self,
        stmt: &WriteStmt,
        params: &[(String, Expr)],
    ) -> Result<WriteStmt, String> {
        let WriteStmt::Set {
            binding,
            entity,
            cond,
            fields,
            span,
        } = stmt
        else {
            return Ok(stmt.clone());
        };
        let Some(e) = self.schema.entity(&entity.name) else {
            return Ok(stmt.clone());
        };
        let mut out = fields.clone();
        for (name, expr) in out.iter_mut() {
            let Some(f) = e.field(&name.name) else {
                continue;
            };
            let ty = f.ty.value_ty();
            if !matches!(ty, ScalarTy::Money(_) | ScalarTy::Decimal(_)) {
                continue;
            }
            let resolved = match &*expr {
                Expr::Param { name: p, .. } if p != "caller" => {
                    match params.iter().find(|(n, _)| n == p) {
                        Some((_, v)) => fold_neg_lit(v),
                        None => continue,
                    }
                }
                other => fold_neg_lit(other),
            };
            match ordo_schema::stored_numeric(ty, &resolved) {
                Ok(Some(stored)) => *expr = stored,
                Ok(None) => {}
                // A registered op's own literals were checked at registration
                // (`ordo_plan::writes`), and a caller's value is refused at
                // the door (`invalid_param`, `ordo_api::bind`), so reaching
                // here is a fault in whatever skipped both: internal, and
                // still refused rather than rounded.
                Err(m) => return Err(format!("internal: `{}.{}`: {m}", e.name, f.name)),
            }
        }
        Ok(WriteStmt::Set {
            binding: binding.clone(),
            entity: entity.clone(),
            cond: cond.clone(),
            fields: out,
            span: *span,
        })
    }

    /// **The create half of a converge-or-create's computed value**, as a
    /// literal (owner, 2026-10-31). `sql` is the right-hand side rendered
    /// against the row, the same fragment the `DO UPDATE` half carries; here
    /// it is evaluated against a row of the entity's declared defaults, in
    /// the units the columns store, so `"counters"."n" + 1` reads `0 + 1`.
    /// One renderer for both halves, so the two branches cannot disagree on
    /// what `+` means. Every field the value reads has a default:
    /// [`ordo_plan::writes::create_branch_reads`] refused it otherwise.
    fn create_branch_literal(
        &self,
        db: &dyn WriteDb,
        e: &Entity,
        f: &ordo_schema::Field,
        sql: &str,
        span: Span,
        caller: &str,
    ) -> Result<Expr, String> {
        let mut defaults = Vec::new();
        for d in &e.fields {
            let (false, false, Some(v)) = (d.implicit, d.sealed, &d.default) else {
                continue;
            };
            let v = match ordo_schema::stored_numeric(d.ty.value_ty(), &fold_neg_lit(v)) {
                Ok(Some(stored)) => stored,
                _ => v.clone(),
            };
            let v = fit_json(&e.name, d, eval_expr(&v, &[], caller)?)?;
            defaults.push(format!("{} AS {}", v.sql, q_ident(&d.column())));
        }
        let row = db
            .rows(&format!(
                "SELECT {sql} FROM (SELECT {}) AS {}",
                defaults.join(", "),
                q_ident(&e.name)
            ))?
            .into_iter()
            .next()
            .ok_or_else(|| {
                "internal: a create value over the defaults returned no row".to_string()
            })?;
        folded_literal(row.first().cloned().flatten(), &e.name, f, span)
    }

    /// **Fold every row-free computed right-hand side, once, before the
    /// statement is planned.**
    ///
    /// `"thirsty " + $plant` has an answer before the write runs, and the
    /// write protocol needs answers: the create gate proves a guard against
    /// the would-be row, a sealed field seals plaintext, a unique key
    /// addresses, a ref is checked for visibility, a closed value set is
    /// checked against its ladder, and the command log images what landed.
    /// So the value is obtained *first* and the statement continues as if
    /// the author had written it out — the same trick, and for the same
    /// reason, as [`substitute_bound`]: downstream must not have to learn
    /// about a new kind of value.
    ///
    /// **The database does the folding**, in one `SELECT` on the write's own
    /// connection — inside the transaction on the executor path. That is
    /// what keeps there being exactly one evaluator for the language's
    /// expressions. A host-side interpreter would be a second implementation
    /// of `+`-over-money, of `/`'s scale, of `round`'s half-away-from-zero,
    /// and the two would agree until the day they did not. Expressions are
    /// pure and deterministic (DL §3), so asking is free of consequence.
    fn fold_computed_rhs(
        &self,
        db: &dyn WriteDb,
        stmt: &WriteStmt,
        params: &[(String, Expr)],
        caller: &str,
    ) -> Result<WriteStmt, String> {
        let WriteStmt::Set {
            binding,
            entity,
            cond,
            fields,
            span,
        } = stmt
        else {
            return Ok(stmt.clone());
        };
        // An unknown entity or field is another check's answer
        // (`unknown_source`, `projection_unknown_field`); shadowing it here
        // would report the wrong problem first.
        let Some(e) = self.schema.entity(&entity.name) else {
            return Ok(stmt.clone());
        };
        let label = ordo_plan::writes::stmt_label(&self.schema, stmt);
        let mut asks: Vec<(usize, String)> = Vec::new();
        for (i, (name, expr)) in fields.iter().enumerate() {
            let Some(f) = e.field(&name.name) else {
                continue;
            };
            if is_held_value(expr, params, f.ty.value_ty()) {
                continue;
            }
            // `rev` is a stated belief, not a value, and the platform's
            // other implicit columns are never written; both are refused by
            // name further down the protocol.
            if f.implicit {
                continue;
            }
            if let Rhs::RowFree(sql) = self.classify_rhs(e, f, expr, params, caller, label)? {
                asks.push((i, sql));
            }
        }
        if asks.is_empty() {
            return Ok(stmt.clone());
        }
        // One round trip for the whole statement: the values are
        // independent of each other and of the row, so asking for them
        // separately would only multiply the trips.
        let select = format!(
            "SELECT {}",
            asks.iter()
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let row = db
            .rows(&select)?
            .into_iter()
            .next()
            .ok_or_else(|| "internal: folding a write value returned no row".to_string())?;
        let mut fields = fields.clone();
        for (k, (i, _)) in asks.iter().enumerate() {
            let f = e.field(&fields[*i].0.name).expect("resolved above");
            let sp = fields[*i].1.span();
            fields[*i].1 = folded_literal(row.get(k).cloned().flatten(), &e.name, f, sp)?;
        }
        Ok(WriteStmt::Set {
            binding: binding.clone(),
            entity: entity.clone(),
            cond: cond.clone(),
            fields,
            span: *span,
        })
    }

    /// The `SET` list for one row. `row` is the id of the row the values
    /// will be written to, because a sealed value is bound to it: the list
    /// is only valid for that row, and a statement over many rows takes
    /// one list per row.
    fn assignments(
        &self,
        e: &Entity,
        fields: &[(ordo_syntax::Ident, Expr)],
        params: &[(String, Expr)],
        caller: &str,
        label: &str,
        row: &str,
    ) -> Result<String, String> {
        let mut parts = Vec::new();
        for (name, expr) in fields {
            let f = e.field(&name.name).ok_or_else(|| {
                format!(
                    "projection_unknown_field: `{}` has no field `{}`",
                    e.name, name.name
                )
            })?;
            if f.implicit {
                return Err(format!(
                    "reserved_derived_name: `{}` is written by the platform, never by an op",
                    name.name
                ));
            }
            // **The computed increment, and the whole of what makes it
            // atomic.** `used + 1` becomes `"used" = "teams"."used" + 1`
            // inside the row's own UPDATE, so the read of the current value
            // and the write of the new one are one statement under the row
            // lock this path already took. There is no host-side
            // read-modify-write here to lose an update, and DL §6 asks for
            // exactly that: "computed server-side inside the row's
            // transaction; atomic increments without read-modify-write
            // races."
            match self.classify_rhs(e, f, expr, params, caller, label)? {
                Rhs::Held => {}
                Rhs::RowReading(sql) => {
                    parts.push(format!("{} = {sql}", q_ident(&f.column())));
                    continue;
                }
                // Unreachable: `fold_computed_rhs` runs before any statement
                // is planned and turns every row-free value into a literal.
                // Answered rather than assumed, because the alternative to
                // an answer here is a sealed field written in plaintext.
                Rhs::RowFree(_) => {
                    return Err(format!(
                        "internal: `{}.{}` reached the assignment builder unfolded",
                        e.name, f.name
                    ))
                }
            }
            let v = fit_json(&e.name, f, eval_expr(expr, params, caller)?)?;
            if f.sealed {
                let plain = match &v.gate {
                    CreateVal::Text(t) | CreateVal::Id(t) => Some(t.clone()),
                    CreateVal::Null => None,
                    other => {
                        return Err(format!(
                            "invalid_binding_expr: sealed `{}` takes text, got {other:?}",
                            f.name
                        ))
                    }
                };

                match plain {
                    None => {
                        parts.push(format!("{} = NULL", q_ident(&f.column())));
                        if f.indexed {
                            // Assigning null CLEARS the payload and moves
                            // the digest to the reserved null sentinel —
                            // it does not clear the digest. Clearing it
                            // would let a second row be nulled into the
                            // same unique field without collision, and
                            // would strand the row where equality can
                            // never reach it again.
                            let digest = self
                                .keys
                                .blind_index(
                                    self.space(),
                                    &e.name,
                                    &f.name,
                                    ordo_crypto::BlindValue::Null,
                                )
                                .map_err(|er| format!("blind index: {er}"))?;
                            parts.push(format!(
                                "{} = '\\x{}'",
                                q_ident(&f.index_column()),
                                ordo_crypto::hex(&digest)
                            ));
                        }
                    }
                    Some(t) => {
                        let lease = self
                            .keys
                            .release(self.space(), ordo_crypto::Purpose::Write, "write")
                            .map_err(|er| format!("key release: {er}"))?;
                        let sealed = self
                            .keys
                            .seal_at(
                                &lease,
                                &e.name,
                                &f.name,
                                ordo_crypto::Anchor::Row(row),
                                t.as_bytes(),
                            )
                            .map_err(|er| format!("seal: {er}"))?;
                        parts.push(format!(
                            "{} = '\\x{}'",
                            q_ident(&f.column()),
                            ordo_crypto::hex(&sealed)
                        ));
                        if f.indexed {
                            let normalized = match f.normalize.as_deref() {
                                Some("lower") => t.to_lowercase(),
                                _ => t.clone(),
                            };
                            let digest = self
                                .keys
                                .blind_index(
                                    self.space(),
                                    &e.name,
                                    &f.name,
                                    ordo_crypto::BlindValue::Text(&normalized),
                                )
                                .map_err(|er| format!("blind index: {er}"))?;
                            parts.push(format!(
                                "{} = '\\x{}'",
                                q_ident(&f.index_column()),
                                ordo_crypto::hex(&digest)
                            ));
                        }
                    }
                }
            } else {
                check_enum(&e.name, f, &v.gate)?;
                check_text_max(&e.name, f, &v.gate)?;
                parts.push(format!("{} = {}", q_ident(&f.column()), v.sql));
            }
        }
        Ok(parts.join(", "))
    }
}

/// [`row_image`](Fixture::row_image)'s [`ordo_schema::image::SealSink`]:
/// sealing under this write's own custody, staged into `log.payloads`
/// rather than written immediately (`Fixture::seal_image_ref`'s own doc
/// explains why staging rather than writing is what makes a payload free
/// to discard when the write that would have owned it never commits).
struct ConformanceSealSink<'a> {
    fixture: &'a Fixture,
    log: &'a mut WriteLog,
}

impl ordo_schema::image::SealSink for ConformanceSealSink<'_> {
    fn seal_ref(
        &mut self,
        entity: &str,
        field: &str,
        anchor: &str,
        envelope: &[u8],
    ) -> Result<LogValue, ordo_schema::image::ImageError> {
        self.fixture
            .seal_image_ref(self.log, entity, field, anchor, envelope)
            .map_err(ordo_schema::image::ImageError::Sink)
    }
}

/// A `json` field's value, checked and cast (28b).
///
/// A document reaches the write path as its JSON text: a caller's value in
/// the string literal the call carries it in, or a default (`{}`, `[]`).
/// The binder has already checked a parameter's text parses; this is the
/// last gate before the text becomes a `jsonb` column, so a value that is
/// not JSON is a refusal here rather than a 500 from Postgres. The cast is
/// explicit so the literal never depends on Postgres inferring the
/// column's type from context.
///
/// The gate value is the text, never an id: a document that happens to
/// look like one (`"a_b"`) is still a document. A sealed field seals the
/// same text, which is why a sealed `json` is stored as a sealed `text` is.
fn fit_json(entity: &str, f: &ordo_schema::Field, v: EvaledVal) -> Result<EvaledVal, String> {
    if f.ty.value_ty() != ScalarTy::Json {
        return Ok(v);
    }
    match v.gate {
        CreateVal::Null => Ok(v),
        CreateVal::Text(t) | CreateVal::Id(t) => {
            let Ok(doc) = serde_json::from_str::<serde_json::Value>(&t) else {
                return Err(format!(
                    "invalid_binding_expr: `{entity}.{}` is `json`, and the value written to it \
                     is not a JSON document",
                    f.name
                ));
            };
            // The door's limits, by the door's checker: the last gate every
            // path crosses, so an imported document and a literal in a
            // routine body obey what a parameter does.
            if let Err(why) = ordo_schema::json::check(&doc, &ordo_schema::json::limits()) {
                let class = match why {
                    ordo_schema::json::JsonRefusal::Nul => "invalid_param",
                    _ => "bound_violation",
                };
                return Err(format!("{class}: `{entity}.{}` {why}", f.name));
            }
            Ok(EvaledVal {
                sql: format!("{}::jsonb", sql_str(&t)),
                gate: CreateVal::Text(t),
            })
        }
        other => Err(format!(
            "invalid_binding_expr: `{entity}.{}` is `json` and takes a document, got {other:?}",
            f.name
        )),
    }
}

/// `title: text(64)` — refuse a value longer than the field's declared
/// bound, in characters, before any SQL runs.
///
/// Same shape and same reason as [`check_enum`]: the DDL's
/// `CHECK (char_length(...) <= n)` is the backstop for what does not come
/// through this path, and for a **sealed** field there is no backstop at
/// all — the column holds ciphertext, so this refusal is the only place
/// the bound can be kept. The door already refuses an over-long
/// *parameter* (`bound_violation`); this catches a value that reached the
/// field another way, such as a literal in the braces or a `+` over
/// parameters.
///
/// The class is `check_violation`, the class the database's own backstop
/// would raise: the bound is one of the entity's row invariants
/// (`ordo_schema::check::text_bound_checks`), so an open and a sealed field
/// answer alike. `bound_violation` stays the door's word for a parameter;
/// the executor may not say it (`ErrorCode::executor_raisable`).
fn check_text_max(entity: &str, f: &ordo_schema::Field, v: &CreateVal) -> Result<(), String> {
    let Some(max) = f.text_max else {
        return Ok(());
    };
    let CreateVal::Text(t) = v else {
        return Ok(());
    };
    let len = t.chars().count();
    if len <= max as usize {
        return Ok(());
    }
    Err(format!(
        "check_violation: `{entity}.{}` is `text({max})` and the value has {len} characters \
         (row invariant `{entity}$len${}<={max}`)",
        f.name, f.name,
    ))
}

/// A value landing in a field pinned to a closed set must be in it.
///
/// The class is **`param_type_mismatch`** — "a binding expression's value
/// does not have its declared type", which is precisely what this is: the
/// field's declared type is `enum("read", "write", "admin")` and `"root"`
/// is not a value of it. No new wire class is minted for this, and none
/// should be: the inventory is closed, this refusal is 400 for the reason
/// every other member of that family is (the statement refused it, not the
/// stored data, so no state will ever make the identical call succeed),
/// and a caller reading `param_type_mismatch` alongside the ladder knows
/// exactly what to send.
///
/// The message names the ladder because the caller cannot guess it: it is
/// derived, not declared, and a bare "constraint violated" would send them
/// to the database to read a `CHECK`. The refusal happens before any SQL
/// runs — the `CHECK` in the DDL is the backstop for everything that does
/// not come through this path (a restore, a hand-written statement), not
/// the primary answer.
///
/// This is what makes the acl level ordering **total**. A guard that
/// enumerates the qualifying levels *excludes* a value outside the ladder
/// rather than refusing it, so `level: "root"` would be a row that reads
/// nothing and writes nothing and looks like a grant — the quietest
/// possible authorization bug. The ladder is closed, or the ordering is
/// not an ordering.
fn check_enum(entity: &str, f: &ordo_schema::Field, v: &CreateVal) -> Result<(), String> {
    let Some(values) = &f.enum_values else {
        return Ok(());
    };
    // A null in an optional field is absence, not a value outside the set.
    let (CreateVal::Text(t) | CreateVal::Id(t)) = v else {
        return Ok(());
    };
    if values.iter().any(|x| x == t) {
        return Ok(());
    }
    Err(format!(
        "param_type_mismatch: `{t}` is not one of `{entity}.{}`'s declared values ({})",
        f.name,
        values
            .iter()
            .map(|v| format!("\"{v}\""))
            .collect::<Vec<_>>()
            .join(", "),
    ))
}

struct PlannedStmt {
    summary: Value,
    sql: Vec<String>,
    commands: Vec<PendingCommand>,
    /// Rows whose write guard must hold **after** the mutation. Proving
    /// the guard against the pre-image alone lets a caller move a row they
    /// legitimately control ACROSS the authorization boundary — reassign a
    /// child's parent ref to one they do not own and the row (and
    /// everything reachable through it) lands in someone else's space with
    /// the caller still attached. The guard has to hold for the row as it
    /// will BE, not only as it was.
    post: Vec<PostRow>,
}

/// A row the post-image obligation is owed on.
enum PostRow {
    /// Located by id: the statement knows exactly which row it changed.
    Id { entity: String, id: String },
    /// Located by unique key: converge-or-create hands the branch decision
    /// to the database (`ON CONFLICT`), so the row that ends up holding
    /// the key is the row whose guard must hold — even when a concurrent
    /// insert made that a different row than the statement planned for.
    Key {
        entity: String,
        columns: Vec<String>,
        literals: Vec<String>,
        /// The id this statement believed it would create, when it was on
        /// the create branch. Another id there means it lost the race, and
        /// for a create-once write losing that race IS the answer.
        expect: Option<String>,
    },
}

/// The addressing half of a converge-or-create, resolved before any guard
/// runs: which unique key the braces cover, where it lives in SQL, whether
/// it already names a row, and what the caller believes about it.
struct Upsert {
    /// The key's fields, in declared order. One for the ordinary
    /// single-column key; more for a composite one (`unique (board,
    /// member)`, the shape `acl` derives), where the address is the whole
    /// tuple and no proper subset of it names a row.
    key: Vec<String>,
    columns: Vec<String>,
    literals: Vec<String>,
    existing: Option<String>,
    belief: RevBelief,
}

impl Upsert {
    /// The key as a caller reads it in a refusal: `email`, or
    /// `board, member`.
    fn name(&self) -> String {
        self.key.join(", ")
    }

    /// `rev: null` — the write must create; a matching row is
    /// `expected_absent`, never a converge.
    fn create_once(&self) -> bool {
        matches!(self.belief, Some(None))
    }
}

struct PendingCommand {
    entity_name: String,
    row_id: String,
    kind: CommandKind,
    before: Option<Image>,
    after: Option<Image>,
}

/// A condition that is nothing but `field == <literal or param>`
/// equalities joined by `and`, as (field, value) pairs in source order.
///
/// `None` for anything else — an `or`, a `not`, a bare bool, a dot-path, a
/// comparison against another field, a text or tree operator. The test is
/// **structural**, like every other mode decision in this file: whether a
/// statement addresses a row is a property of the statement, decided
/// before any value is looked at.
fn flat_equalities(c: &Condition) -> Option<Vec<(String, &Expr)>> {
    match c {
        Condition::Compare {
            op: ordo_syntax::CmpOp::Eq,
            lhs: Expr::Path { segments, .. },
            rhs,
            ..
        } if segments.len() == 1
            && matches!(
                rhs,
                Expr::Param { .. } | Expr::StrLit { .. } | Expr::NullLit { .. }
            ) =>
        {
            Some(vec![(segments[0].name.clone(), rhs)])
        }
        Condition::And { items, .. } => {
            let mut out = Vec::with_capacity(items.len());
            for i in items {
                out.extend(flat_equalities(i)?);
            }
            // A field named twice is two claims about one column; it is
            // not an address, whatever the values turn out to be.
            let mut seen: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
            seen.sort_unstable();
            let before = seen.len();
            seen.dedup();
            (seen.len() == before).then_some(out)
        }
        _ => None,
    }
}

/// `id == <literal-or-param>` and nothing else — the addressed shape.
///
/// The shape is decided **structurally**, before the value is looked at,
/// because the mode is static (§6 rule 1): `id == $x` is an addressed
/// write whatever `$x` turns out to be. A null bound value is therefore
/// `null_key_address` (rule 8) and not a silent demotion to the bulk path,
/// where `id == null` would select nothing and answer `updated: 0` — a
/// write that addressed nothing and said it succeeded.
/// The mode derivation's **shape** half: is this condition `id == …`?
///
/// [`addressed_id`] answers the same question by evaluating the right-hand
/// side, which needs a caller (`id == $caller` is a legal address) and can
/// fail on the caller's input. The anonymous path needs only the shape: it
/// never learns *which* row was addressed, and must not, so it asks here.
fn addresses_id(c: &Condition) -> bool {
    matches!(
        c,
        Condition::Compare {
            op: ordo_syntax::CmpOp::Eq,
            lhs: Expr::Path { segments, .. },
            ..
        } if segments.len() == 1 && segments[0].name == "id"
    )
}

fn addressed_id(
    c: &Condition,
    params: &[(String, Expr)],
    caller: &str,
) -> Result<Option<String>, String> {
    let Condition::Compare {
        op: ordo_syntax::CmpOp::Eq,
        lhs: Expr::Path { segments, .. },
        rhs,
        ..
    } = c
    else {
        return Ok(None);
    };
    if segments.len() != 1 || segments[0].name != "id" {
        return Ok(None);
    }
    match eval_expr(rhs, params, caller)?.gate {
        CreateVal::Id(v) | CreateVal::Text(v) => Ok(Some(v)),
        CreateVal::Null => Err(
            "null_key_address: `id == null` addresses no row — this write is addressed, and \
             a null address is not one"
                .into(),
        ),
        other => Err(format!("param_type_mismatch: `id` is an id, got {other:?}")),
    }
}

/// The **row descriptor** a `let x = set E { … }` binds: *"`id`, `rev`, and
/// the fields that statement wrote"* (DL §3, "Naming a freshly created row
/// in the same op"; R-G19).
///
/// The members are kept as **expressions**, not as evaluated values, and
/// the ones that come from the braces are the create's own expressions
/// unchanged. That is what makes `x.name` and the `name:` it came from
/// provably the same value: they are the same node, evaluated by the same
/// `eval_expr` against the same parameter bindings, and there is no second
/// evaluation path that could disagree. Only `id` and `rev` are minted
/// here, because only they are the platform's — a create's `rev` is 1, and
/// the id is the one the mint just handed the insert.
fn descriptor(id: &str, fields: &[(ordo_syntax::Ident, Expr)]) -> Vec<(String, Expr)> {
    let sp = Span::new(0, 0, 1, 1);
    let mut out = vec![
        (
            "id".to_string(),
            Expr::StrLit {
                value: id.to_string(),
                span: sp,
            },
        ),
        ("rev".to_string(), Expr::IntLit { value: 1, span: sp }),
    ];
    out.extend(fields.iter().map(|(n, v)| (n.name.clone(), v.clone())));
    out
}

/// Replace every `x.member` in a statement with the bound descriptor's
/// expression for `member`.
///
/// Done as an AST rewrite *before* planning rather than as a new case
/// inside `eval_expr`, deliberately: a descriptor reference is not a new
/// kind of write value, it is a name for one that already exists, and the
/// write protocol downstream — ref visibility (G12), the create gate
/// (G10a), assignments, images, the command log — must not need to learn
/// about it. After this rewrite, a statement that named a bound row is
/// indistinguishable from one the caller wrote the id into by hand, which
/// is precisely what R-G19 says it means.
///
/// Unresolvable references are a registration-time error
/// (`invalid_binding_expr`, `ordo_plan::writes::check_bindings`); reaching
/// one here means an unregistered statement was planned directly, so it is
/// answered rather than assumed away.
fn substitute_bound(
    stmt: &WriteStmt,
    bound: &[(String, Vec<(String, Expr)>)],
) -> Result<WriteStmt, String> {
    if bound.is_empty() {
        return Ok(stmt.clone());
    }
    fn expr(e: &Expr, bound: &[(String, Vec<(String, Expr)>)]) -> Result<Expr, String> {
        Ok(match e {
            Expr::Path { segments, .. } if segments.len() == 2 => {
                match bound.iter().find(|(n, _)| *n == segments[0].name) {
                    None => e.clone(),
                    Some((_, members)) => members
                        .iter()
                        .find(|(m, _)| *m == segments[1].name)
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| {
                            format!(
                                "invalid_binding_expr: `{}` did not write `{}`, so its \
                                 descriptor does not carry it",
                                segments[0].name, segments[1].name
                            )
                        })?,
                }
            }
            Expr::Binary { op, lhs, rhs, span } => Expr::Binary {
                op: *op,
                lhs: Box::new(expr(lhs, bound)?),
                rhs: Box::new(expr(rhs, bound)?),
                span: *span,
            },
            Expr::Neg { expr: e, span } => Expr::Neg {
                expr: Box::new(expr(e, bound)?),
                span: *span,
            },
            Expr::FnCall { name, args, span } => Expr::FnCall {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|a| expr(a, bound))
                    .collect::<Result<_, _>>()?,
                span: *span,
            },
            other => other.clone(),
        })
    }
    // A condition can name a bound row too — just never the condition of
    // the statement that bound it, which is the scope rule and is enforced
    // at registration.
    fn cond(c: &Condition, bound: &[(String, Vec<(String, Expr)>)]) -> Result<Condition, String> {
        Ok(match c {
            Condition::Compare { op, lhs, rhs, span } => Condition::Compare {
                op: *op,
                lhs: expr(lhs, bound)?,
                rhs: expr(rhs, bound)?,
                span: *span,
            },
            Condition::In {
                expr: e,
                list,
                span,
            } => Condition::In {
                expr: expr(e, bound)?,
                list: list
                    .iter()
                    .map(|x| expr(x, bound))
                    .collect::<Result<_, _>>()?,
                span: *span,
            },
            Condition::And { items, span } => Condition::And {
                items: items
                    .iter()
                    .map(|i| cond(i, bound))
                    .collect::<Result<_, _>>()?,
                span: *span,
            },
            Condition::Or { items, span } => Condition::Or {
                items: items
                    .iter()
                    .map(|i| cond(i, bound))
                    .collect::<Result<_, _>>()?,
                span: *span,
            },
            Condition::Not { inner, span } => Condition::Not {
                inner: Box::new(cond(inner, bound)?),
                span: *span,
            },
            other => other.clone(),
        })
    }
    Ok(match stmt {
        WriteStmt::Set {
            binding,
            entity,
            cond: c,
            fields,
            span,
        } => WriteStmt::Set {
            binding: binding.clone(),
            entity: entity.clone(),
            cond: c.as_ref().map(|c| cond(c, bound)).transpose()?,
            fields: fields
                .iter()
                .map(|(n, v)| Ok((n.clone(), expr(v, bound)?)))
                .collect::<Result<_, String>>()?,
            span: *span,
        },
        WriteStmt::Del {
            entity,
            cond: c,
            subtree,
            span,
        } => WriteStmt::Del {
            entity: entity.clone(),
            cond: cond(c, bound)?,
            subtree: *subtree,
            span: *span,
        },
    })
}

/// Literal / `$param` / `$caller` evaluation — the whole write-RHS story
/// for this slice. Arithmetic and field references in write RHS are F5.
fn eval_expr(e: &Expr, params: &[(String, Expr)], caller: &str) -> Result<EvaledVal, String> {
    Ok(match e {
        Expr::StrLit { value, .. } => EvaledVal {
            gate: if value.contains('_') && value.len() < 40 && !value.contains(' ') {
                // Ids and texts are both string literals in source; the
                // gate only compares, so a permissive guess is safe.
                CreateVal::Id(value.clone())
            } else {
                CreateVal::Text(value.clone())
            },
            sql: sql_str(value),
        },
        Expr::IntLit { value, .. } => EvaledVal {
            gate: CreateVal::Int(*value as i64),
            sql: value.to_string(),
        },
        // Only a `decimal(p)` field holds one: `fit_scaled_literals` has
        // already turned every money literal into whole minor units, and
        // `is_held_value` answers `false` for a decimal anywhere else, so
        // a decimal never reaches an integral column from here.
        Expr::DecimalLit { digits, .. } => EvaledVal {
            gate: CreateVal::Decimal(digits.clone()),
            sql: digits.clone(),
        },
        Expr::BoolLit { value, .. } => EvaledVal {
            gate: CreateVal::Bool(*value),
            sql: if *value {
                "true".into()
            } else {
                "false".into()
            },
        },
        Expr::NullLit { .. } => EvaledVal {
            gate: CreateVal::Null,
            sql: "NULL".into(),
        },
        Expr::DateLit { y, m, d, .. } => EvaledVal {
            gate: CreateVal::Text(format!("{y:04}-{m:02}-{d:02}")),
            sql: format!("DATE '{y:04}-{m:02}-{d:02}'"),
        },
        Expr::DatetimeLit {
            y,
            m,
            d,
            hh,
            mm,
            ss,
            ..
        } => {
            let at = format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z");
            EvaledVal {
                sql: format!("TIMESTAMPTZ '{at}'"),
                gate: CreateVal::Text(at),
            }
        }
        Expr::Param { name, .. } if name == "caller" => EvaledVal {
            gate: CreateVal::Id(caller.to_string()),
            sql: sql_str(caller),
        },
        Expr::Param { name, span } => {
            let (_, v) = params
                .iter()
                .find(|(n, _)| n == name)
                .ok_or_else(|| format!("unbound_required_param: `${name}` at {span}"))?;
            eval_expr(v, &[], caller)?
        }
        // Wrapped rather than one long literal because rustfmt 1.9's
        // `--check` and its formatter disagree about the un-wrapped form:
        // the formatter writes `other => return Err(...)` and the check
        // demands the braced arm, so `cargo fmt` could not make `cargo fmt
        // --check` pass. CI runs the check.
        other => {
            let m = "invalid_binding_expr: write values are literals, \
                     params, or $caller";
            return Err(format!("{m} (got {other:?})"));
        }
    })
}

fn log_value(v: &CreateVal) -> LogValue {
    match v {
        CreateVal::Id(s) => LogValue::Id(s.clone()),
        CreateVal::Text(s) => LogValue::Text(s.clone()),
        CreateVal::Bool(b) => LogValue::Bool(*b),
        CreateVal::Int(i) => LogValue::Int(*i),
        // The chain's decimal tag, in its one canonical spelling (27ak).
        // A spelling that is not a decimal cannot reach here (the binder
        // and the door refuse one); text is the honest record if it did.
        CreateVal::Decimal(d) => ordo_commandlog::value::Decimal::parse(d)
            .map(LogValue::Decimal)
            .unwrap_or_else(|| LogValue::Text(d.clone())),
        CreateVal::Null => LogValue::Null,
    }
}

/// Is this right-hand side a value the platform already holds — the closed
/// set [`eval_expr`] evaluates?
///
/// **This predicate and `eval_expr`'s arms are one list and must stay one
/// list**: everything that answers `true` here must have an arm there, or a
/// value the write path believed it could hold reaches the closed-set
/// evaluator and is refused as if the author had written an expression.
/// Anything else is a *computed* right-hand side and takes the path
/// [`Fixture::classify_computed`] describes.
///
/// `ty` is the field the value lands in. A decimal literal is held only by
/// a `decimal(p)` field (27ak): anywhere else it is a value the compiler
/// must type, so it takes the computed path and meets `implicit_rounding`
/// there rather than reaching an integral column as `29.99`.
fn is_held_value(e: &Expr, params: &[(String, Expr)], ty: ScalarTy) -> bool {
    match e {
        Expr::StrLit { .. }
        | Expr::IntLit { .. }
        | Expr::BoolLit { .. }
        | Expr::NullLit { .. }
        | Expr::DateLit { .. }
        | Expr::DatetimeLit { .. } => true,
        Expr::DecimalLit { .. } => matches!(ty, ScalarTy::Decimal(_)),
        Expr::Param { name, .. } if name == "caller" => true,
        Expr::Param { name, .. } => params
            .iter()
            .find(|(n, _)| n == name)
            .is_some_and(|(_, v)| is_held_value(v, &[], ty)),
        _ => false,
    }
}

/// The database's answer to a folded right-hand side, back as the literal
/// the write protocol would have had if the author had written the value
/// out.
///
/// The target field's **declared** type decides the literal's form — never
/// the shape of the returned string. Guessing from the text is how
/// `"2026-01-01"` becomes a date nobody asked for and how an id-looking text
/// becomes an id; the schema already knows, and it is the only thing that
/// does.
fn folded_literal(
    value: Option<String>,
    entity: &str,
    f: &ordo_schema::Field,
    span: Span,
) -> Result<Expr, String> {
    use ScalarTy::*;
    let Some(v) = value else {
        return Ok(Expr::NullLit { span });
    };
    Ok(match f.ty.value_ty() {
        // A document comes back as Postgres prints the `jsonb`, and rides
        // on as that text (28b).
        Text | Id | Json => Expr::StrLit { value: v, span },
        Int | Money(_) | Duration(_) => Expr::IntLit {
            value: v.parse::<i128>().map_err(|_| {
                format!(
                    "internal: `{entity}.{}` is `{}` and its computed value came back as \
                     `{v}`, which is not a whole number",
                    f.name,
                    f.ty.value_ty().as_str()
                )
            })?,
            span,
        },
        // A `decimal(p)` column's value, as Postgres prints it at its
        // scale (27ak).
        Decimal(_) => Expr::DecimalLit { digits: v, span },
        // The text protocol's booleans, from psql and from the driver
        // alike.
        Bool => Expr::BoolLit {
            value: matches!(v.as_str(), "t" | "true"),
            span,
        },
        Date => {
            let p: Vec<&str> = v.split('-').collect();
            let [y, m, d] = p.as_slice() else {
                return Err(format!(
                    "internal: `{entity}.{}` is a date and its computed value came back as \
                     `{v}`",
                    f.name
                ));
            };
            Expr::DateLit {
                y: y.parse().map_err(|_| "internal: bad year".to_string())?,
                m: m.parse().map_err(|_| "internal: bad month".to_string())?,
                d: d.parse().map_err(|_| "internal: bad day".to_string())?,
                span,
            }
        }
        // **Scoped out, by name rather than by a wrong literal.** A
        // computed `datetime` has no spelling the database's answer
        // round-trips through (a written literal is held, `eval_expr`
        // renders it), and `blob`/`uri` are content:
        // a handle is minted by an upload and an address is validated at
        // the door, so neither is something an expression may compute.
        // `decimal` is expression-only (DL §2) and is never a column, so
        // reaching it here means a type check upstream let one through.
        other => {
            return Err(format!(
                "invalid_binding_expr: `{entity}.{}` is `{}`, and a computed right-hand side \
                 has no value of that type to become — write the value out, or pass it as a \
                 parameter",
                f.name,
                other.as_str()
            ))
        }
    })
}

pub(crate) fn q_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub(crate) fn sql_str(s: &str) -> String {
    // 9u: the last gate before a text value becomes part of a statement, and
    // therefore the one the spec's word "on write" names. Every string this
    // path renders should ALREADY be NFC — literals were normalized by the
    // lexer and caller values by the door — so this is defence in depth
    // rather than the mechanism, and it is cheap: `nfc` returns an
    // already-canonical value without reallocating.
    //
    // It is kept anyway because the conformance harness does not go through
    // `ordo-api` at all. A guarantee that lived only at the door would be
    // untested by the suite that proves the platform's behaviour, and any
    // future caller path would escape it silently — which is exactly how DL
    // §2's settled rule came to have no implementation for this long.
    format!("'{}'", ordo_syntax::nfc(s).replace('\'', "''"))
}

fn first_col(rows: Vec<Vec<Option<String>>>) -> Vec<String> {
    rows.into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect()
}

/// The entity a write statement targets, whatever its verb.
fn stmt_entity(stmt: &WriteStmt) -> Option<&str> {
    match stmt {
        WriteStmt::Set { entity, .. } | WriteStmt::Del { entity, .. } => Some(&entity.name),
    }
}

#[cfg(test)]
mod minting {
    use super::WriteLog;
    use std::sync::Arc;

    fn deployment_log(prefix: &str) -> WriteLog {
        WriteLog::serving(prefix, Arc::new(|| 0))
    }

    /// 25e. A deployment minted `note_…_9` and then `note_…_10`, which
    /// sorts first, so a list ordered by id was not ordered by age.
    #[test]
    fn a_deployments_minted_ids_sort_by_age_past_ten_and_across_logs() {
        let mut first = deployment_log("ffffffffffffffff_");
        let minted: Vec<String> = (0..25).map(|_| first.mint()).collect();
        let mut sorted = minted.clone();
        sorted.sort();
        assert_eq!(sorted, minted, "one log's ids sort in the order minted");

        // A later log, with a prefix that sorts lower, still sorts after.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut later = deployment_log("0000000000000000_");
        let next = later.mint();
        assert!(
            minted.iter().all(|m| m < &next),
            "{next} sorts after every id minted before it"
        );
    }
}
