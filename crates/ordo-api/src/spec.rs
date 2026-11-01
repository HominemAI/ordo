//! The boundary types: what the transport layer needs from a compiled op
//! artifact, and the traits the rest of the platform fulfills. Nothing here
//! knows about HTTP.

use futures::future::BoxFuture;
use tokio::sync::broadcast;

use crate::page::PageCursor;
use crate::refine::Refinement;

/// The op's derived kind, mirrored onto HTTP by the router. Read ops page,
/// refine, and go live (`GET`); write ops — any `set`, including mixed
/// set+del — carry idempotency keys and mode labels (`POST`); pure-`del`
/// ops are `Delete` (`DELETE`). The split drives the whole request pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Read,
    Write,
    Delete,
}

/// Closed parameter type inventory, mirroring the language (§2 of the
/// definition language). `money` is a decimal string on the wire — exact,
/// never a float.
///
/// # It mirrors the language; it is not a copy of it
///
/// Two members of `ScalarTy` were deliberately absent, and their absence
/// was the inventory saying something rather than forgetting something.
///
/// - **`blob`** is refused as an *op* parameter by the compiler
///   (`ordo_sema`'s `blob_parameter`). Bytes are not a JSON wire value;
///   they arrive through the blob door, which is why that door exists.
///   An op that took a handle would be a caller naming an object it did
///   not upload. A **function** takes one ([`ParamTy::Blob`], increment 2
///   of `plan-backend-functions.md`): the bytes still arrive through the
///   blob door, the function gets the handle, and it reads the file as
///   the caller, through the same `std.$blob.get` check and row guard as
///   `GET /blob/{handle}`. So a handle the caller names grants nothing
///   the caller could not read already.
///
/// [`Datetime`](ParamTy::Datetime) is present and **was not**, and that was
/// a real defect rather than a tidy gap: `$t: datetime` parsed, and
/// `ordo_host::param_ty`'s wildcard arm bound it at the door as *unbounded
/// text* with no ISO-8601 validation. The wildcard is why nobody noticed;
/// it is now an exhaustive match, so the next type to be added to the
/// language cannot repeat it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamTy {
    Int,
    /// A decimal string in major units, with at most `places` digits after
    /// the point (27ak): `"29.99"` for a `money(2)`. The door refuses a
    /// finer value rather than rounding it, and the executor stores whole
    /// minor units.
    Money {
        places: u8,
    },
    /// A `decimal(p)`: an exact decimal string with at most `places`
    /// digits after the point (27ak). Refused rather than rounded when
    /// finer.
    Decimal {
        places: u8,
    },
    Text {
        max_len: Option<u32>,
    },
    Date,
    /// `YYYY-MM-DDTHH:MM:SSZ` — UTC, the `Z` mandatory, exactly as
    /// `api-engine-design.md` §5's wire table now says.
    Datetime,
    Bool,
    Id,
    Duration,
    /// An address, with no custody (DL §2, Content types).
    ///
    /// Its own member rather than `Text`, for the reason `datetime` is:
    /// binding an address as unbounded text is a validation the door
    /// silently does not do. A `uri` parameter reaches a guard, a filter,
    /// a stored column and — the whole point of the type — something a
    /// consent page tells a user the platform will *reference*. That it is
    /// syntactically an address is worth one check at the door.
    Uri,
    /// A JSON document (28b): any JSON value, carried as JSON. The door
    /// checks it against the deployment's size and depth limits
    /// ([`crate::json::JsonLimits`]); the type itself bounds nothing.
    Json,
    /// A file handle from the blob door, as 32 hex characters. A function
    /// parameter only; see above for why an op never has one.
    Blob,
}

/// Declared parameter bounds, enforced at the door before planning.
/// Values are canonical strings: exact decimals for `int`/`money`,
/// `YYYY-MM-DD` for `date`. Other types take no bounds in v1.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
    pub min: Option<String>,
    pub max: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: String,
    pub ty: ParamTy,
    pub required: bool,
    pub bounds: Option<Bounds>,
}

/// The cost lattice, as the door sees it.
///
/// # Why this enum exists here at all
///
/// The classes are `ordo_plan::cost::CostClass`'s: the compiler classifies
/// every plan at registration and the catalog prints the word. This crate
/// depends on no sibling crate (see the crate docs — `OpRegistry`,
/// `OpExecutor` and friends are the boundary), so the name has to be
/// re-declared to travel on an [`OpSpec`].
///
/// **Nothing here classifies anything.** Every value on a spec was computed
/// by `ordo_plan::cost` from the op's own plan and handed over; the door's
/// whole arithmetic is [`CostClass::join`], which is §7.1's `⊔`. That is the
/// difference between one cost model with two spellings and two cost models
/// that can disagree — and the spellings are held together mechanically:
/// `ordo-host`'s `cost_class_inventory_matches_the_planner` walks both `ALL`
/// lists and both joins over the whole cross product.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostClass {
    Sql,
    BlindIndex,
    BoundedHost,
    UnboundedHost,
}

impl CostClass {
    /// The inventory, cheapest first — declaration order **is** the lattice
    /// order [`CostClass::join`] ranks by.
    pub const ALL: &'static [CostClass] = &[
        CostClass::Sql,
        CostClass::BlindIndex,
        CostClass::BoundedHost,
        CostClass::UnboundedHost,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CostClass::Sql => "sql",
            CostClass::BlindIndex => "blind-index",
            CostClass::BoundedHost => "bounded-host",
            CostClass::UnboundedHost => "unbounded-host",
        }
    }

    pub fn from_class(class: &str) -> Option<CostClass> {
        CostClass::ALL.iter().copied().find(|c| c.as_str() == class)
    }

    pub fn rank(self) -> u8 {
        CostClass::ALL
            .iter()
            .position(|c| *c == self)
            .expect("every class is in ALL") as u8
    }

    /// `⊔`: composition costs what its most expensive part costs.
    pub fn join(self, other: CostClass) -> CostClass {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// What the door needs to know about an op's **registered** cost, so it can
/// recompute the **effective** cost of a request (`api-engine-design.md`
/// §7.1, `std-library.md` §2.1 rule 4).
///
/// Every field is the compiler's answer about this op's own plan, not a
/// guess the transport made. The default is the honest one for a spec whose
/// producer knows none of it: cheapest class, no acknowledgment, and **no
/// proven candidate bound** — so a sealed refinement on such an op is
/// refused rather than served on the strength of a fact nobody established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpCost {
    /// The class the op was registered with — `ordo_plan::cost::classify_with`
    /// on its plan, the same word the catalog and the deploy diff carry.
    pub class: CostClass,
    /// The author's `unbounded` acknowledgment. It is what makes serving
    /// unbounded-host work legal: *this op's work grows with the size of the
    /// space, and I mean it.* A refinement envelope cannot acknowledge on
    /// anyone's behalf, so this is read, never inferred.
    pub unbounded_ack: bool,
    /// Is the set a post-everything refinement would be applied to provably
    /// bounded — `ordo_plan::cost::refine_candidate_bound`?
    ///
    /// For a row-shaped op that is the op's own row bound (a host-side
    /// condition is applied after the `take`; open and blind-index
    /// conditions compile into the plan and never consult this). For a
    /// **reduce** it is
    /// the bound of what the reduce EXAMINES, because §7 applies the
    /// condition before the reduce: `count` returns one row over a set
    /// nothing bounded, and that gap is what §7.1's rejection protects.
    pub refine_candidate_bounded: bool,
}

impl Default for OpCost {
    fn default() -> Self {
        OpCost {
            class: CostClass::Sql,
            unbounded_ack: false,
            refine_candidate_bounded: false,
        }
    }
}

/// One output field of the op's result shape — the manifest refinements are
/// validated against.
///
/// The flags carry the per-**field** half of the sealed-refinement rules
/// (`std-library.md` §2.1, `api-engine-design.md` §7.1). Without them
/// [`RefineTier`] is per-*op* only and the door cannot tell a sealed field
/// from an open one, so rules 1–3 have nowhere to be enforced (SH-4 / Y-033):
///
/// - `sealed` — the value lives as ciphertext; a refinement condition on it
///   rides the blind index rather than the value.
/// - `refinable` — whether a condition on this field is accepted at all.
///   **Sealed fields default to `false`**: refinability is opt-in at the
///   declaration, so the capability lands in the manifest and therefore on
///   the consent page, like every other disclosure in this design.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: String,
    pub sealed: bool,
    pub refinable: bool,
    /// The class a refinement **condition** on this field implies —
    /// `ordo_plan::cost::refine_field_class`, computed at registration
    /// against this op's own plan.
    ///
    /// This is the per-field half of §7.1's `class_of(…)`. An open field is
    /// `sql` (an ordinary predicate over values the plan already produced);
    /// a `sealed indexed` field is `blind-index` (the equality rides the
    /// digest); a sealed field with no index is host-side, bounded or not
    /// according to [`OpCost::refine_candidate_bounded`].
    pub refine_class: CostClass,
    /// The field's value type, so the door can check a condition's value
    /// before it reaches the binder (`plan-refinement.md` section 4). A
    /// mismatch is `invalid_refinement`, never a 500 from the executor.
    pub ty: FieldTy,
    /// A `where` condition may name this field. `false` for an output a
    /// condition cannot address: `std.E.count`'s own `n`, whose conditions
    /// validate against `std.E.list`'s manifest instead (`std-library.md`
    /// §2.1).
    pub filter: bool,
    /// The field is in the op's result, so a `pick` may name it. `false`
    /// for a condition-only field: `std.E.list`'s fields carried on
    /// `std.E.count`'s spec.
    pub output: bool,
}

/// The value type of one output field, as a refinement condition sees it.
///
/// It mirrors the language's scalar types the way [`ParamTy`] does, with
/// two differences that are the point of having it:
///
/// - **`Blob`** is here. A blob output can be picked, but it has no order
///   and no meaningful equality, so no condition may name it.
/// - **`Any`** is a field whose producer stated no type: a hand-written spec,
///   or a platform op's untyped column. The door checks only the JSON shape
///   (one scalar, an array for `in`), as it did before types existed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldTy {
    Any,
    Text,
    Int,
    /// Compared as an exact number: a decimal string or an integer, never a
    /// float. A value finer than the field's places is legal in a
    /// comparison (`m <= 2.505`); only a parameter must fit the column.
    Money {
        places: u8,
    },
    Decimal {
        places: u8,
    },
    Bool,
    Date,
    Datetime,
    /// An integer count of `unit`, the language's spelling of it (`ms`, `s`,
    /// `m`, `h`, `d`, `w`, `mo`, `y`). The wire carries a duration output as
    /// that integer, so a condition takes the same number.
    Duration {
        unit: String,
    },
    Id,
    Uri,
    Blob,
    /// A JSON document (28b). It can be picked, and no condition may name
    /// it in v1: the language has no operators over a document's insides.
    Json,
}

impl FieldSpec {
    /// An ordinary open field: refinable, like every open field is, and
    /// `sql` to refine.
    pub fn new(name: impl Into<String>) -> Self {
        FieldSpec {
            name: name.into(),
            sealed: false,
            refinable: true,
            refine_class: CostClass::Sql,
            ty: FieldTy::Any,
            filter: true,
            output: true,
        }
    }

    /// A sealed field with no refinement opt-in — the default for sealed.
    /// Its refinement class is the most expensive one, which costs nothing
    /// to say: [`crate::refine::validate`] refuses the condition before the
    /// class is ever consulted.
    pub fn sealed(name: impl Into<String>) -> Self {
        FieldSpec {
            name: name.into(),
            sealed: true,
            refinable: false,
            refine_class: CostClass::UnboundedHost,
            ty: FieldTy::Any,
            filter: true,
            output: true,
        }
    }

    /// A sealed field the author opted into refining, in the shape the
    /// language spells `sealed indexed refinable`: the equality rides the
    /// blind index, so the class is `blind-index`. Equality-only is enforced
    /// at the door ([`crate::refine::validate`]).
    ///
    /// The real producer computes the class from the plan; this constructor
    /// picks the documented shape's answer so a hand-written spec is a
    /// plausible one. A sealed field with **no** index is
    /// [`FieldSpec::with_refine_class`] over a host class.
    pub fn sealed_refinable(name: impl Into<String>) -> Self {
        FieldSpec {
            name: name.into(),
            sealed: true,
            refinable: true,
            refine_class: CostClass::BlindIndex,
            ty: FieldTy::Any,
            filter: true,
            output: true,
        }
    }

    /// Override the class a condition on this field implies.
    pub fn with_refine_class(mut self, class: CostClass) -> Self {
        self.refine_class = class;
        self
    }

    /// State the field's value type.
    pub fn with_ty(mut self, ty: FieldTy) -> Self {
        self.ty = ty;
        self
    }
}

/// Refinement tiers (tooling plan §B2). `Basic` = Tier 0
/// (eq/ne/lt/le/gt/ge/in/is_null + pick), and every compiled read has it
/// (owner, 2026-10-05: refinement is a standard feature with no off
/// switch). `None` is a write, a platform op, a read that did not bind, and
/// a public read once the public path lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefineTier {
    None,
    Basic,
}

/// Whether a read op can be kept in sync on a device (`plan-replica-feed.md`
/// §1), and if so which output keys its rows.
///
/// Computed at registration from the bound plan, like [`OpCost`], by
/// `ordo_plan::artifact::syncability`; the door reads it when a caller asks
/// to sync an op and never infers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Syncability {
    /// Every row of the result is a stored row of `entity`, and the output
    /// named `id_output` carries that row's id.
    Syncable {
        entity: String,
        id_output: String,
    },
    NotSyncable(NotSyncable),
}

/// Which rule of `plan-replica-feed.md` §1 an op fails. The codes are
/// `ordo_plan::artifact::SyncRule`'s, plus [`NotSyncable::NotARead`], which
/// the planner has no occasion to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotSyncable {
    /// A write or delete: an effect, with no result to hold.
    NotARead,
    /// No single base table anchors the result: a union, a CTE, or a
    /// platform op whose rows no program declared.
    NoAnchor,
    /// A grouped result's rows are synthetic and have no identity.
    Grouped,
    /// A measure: one synthetic row.
    Measure,
    /// `distinct` settles the row set: the rows are output tuples, not stored
    /// rows.
    Distinct,
    /// A many-side source join fans one anchor row into several result rows,
    /// so the anchor's `id` does not key a row.
    ManySideJoin,
}

impl NotSyncable {
    /// The wire and manifest spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            NotSyncable::NotARead => "not_a_read",
            NotSyncable::NoAnchor => "no_anchor",
            NotSyncable::Grouped => "grouped",
            NotSyncable::Measure => "measure",
            NotSyncable::Distinct => "distinct",
            NotSyncable::ManySideJoin => "many_side_join",
        }
    }

    /// The rule, in the words a refusal carries.
    pub fn rule(self) -> &'static str {
        match self {
            NotSyncable::NotARead => "only a read op can be synced",
            NotSyncable::NoAnchor => {
                "a synced op's rows must all come from one entity, and this result has no single anchor"
            }
            NotSyncable::Grouped => {
                "a grouped result's rows are synthetic and have no identity to sync by"
            }
            NotSyncable::Measure => "a measure is one synthetic row with no identity to sync by",
            NotSyncable::Distinct => {
                "a `distinct` result's rows are output tuples rather than stored rows, so they have no identity to sync by"
            }
            NotSyncable::ManySideJoin => {
                "this op joins a many side, so one anchor row becomes several result rows and its `id` does not key a row"
            }
        }
    }
}

/// Everything the transport needs from a compiled artifact. Cloned at
/// dispatch, so in-flight requests are immune to registry swaps.
#[derive(Clone, Debug, PartialEq)]
pub struct OpSpec {
    pub name: String,
    pub version_hash: String,
    pub kind: OpKind,
    pub params: Vec<ParamSpec>,
    pub output_fields: Vec<FieldSpec>,
    pub live_capable: bool,
    pub refinable: RefineTier,
    /// The op reduces rows to a measure (`std.E.count` and friends) rather
    /// than returning them. Aggregate-only ops accept **no sealed refinement
    /// at all**: if you may not read the rows, you may not measure them —
    /// otherwise `count(salary gt 100000)`, `count(salary gt 150000)`, …
    /// binary-searches a sealed value to the dollar with no read grant on it
    /// (`std-library.md` §2.1 rule 3).
    pub aggregate_only: bool,
    /// The compiler's static write-mode label (`creates` / `converges` / …).
    pub mode_label: Option<String>,
    /// The registered cost facts the request-time gate joins against
    /// (§7.1). See [`OpCost`].
    pub cost: OpCost,
    /// Whether the op can be kept in sync on a device. See [`Syncability`].
    pub sync: Syncability,
    /// `Idempotency-Key` is **required** on this op, not optional
    /// (`bootstrap.md` §2 item 44).
    ///
    /// For every other write the header is a caller's choice, and the cost
    /// of omitting it is bounded: a retry after a network failure repeats an
    /// insert, and the row's own key, a `rev` CAS, or a `converge` mode
    /// usually catches it. That bound does not hold for an op whose effect
    /// is **outside the database it writes to**. `std.space.create` mints a
    /// KMS key and creates a database before it activates a registry row, so
    /// a retried create makes a *second* tenant — a second database, a second
    /// master key — and answers with the second one's id. The caller never
    /// learns the first exists, and nothing in the platform reconciles them:
    /// both rows are `active` and correct, and the orphan is only visible to
    /// somebody reading the cluster.
    ///
    /// Declared on the spec rather than listed in the router, because the
    /// router is generated from the registry and has no per-op code by
    /// design — and because whoever declares an op is who knows whether its
    /// effect can be made twice safely.
    pub requires_idempotency_key: bool,
    /// Whether a visitor with no token may call this op
    /// (`design-public-path.md` §1). Set by the compiler from `op … public`,
    /// which it accepts only when the op reads public fields alone. The door
    /// reads it in the **active** artifact on every call, so an op that stops
    /// being public stops answering visitors the moment the new version is
    /// served.
    pub public: PublicMode,
}

/// How a visitor with no token may reach an op (`design-public-path.md` §1).
///
/// `None` is every op that is not declared `public`, and every platform op:
/// a tokenless call to it answers the door's one `access_denied`, byte for
/// byte. `Read` is a public read, served as the reserved anonymous principal
/// in the deployment's space, through the public cache. `Write` is a public
/// write: plain creates only (the compiler proves it), run through the
/// ordinary checked and logged write pipeline as the anonymous principal,
/// behind the per-op hourly cap and the human check
/// (`design-public-path.md` §5). Never cached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PublicMode {
    #[default]
    None,
    Read,
    Write,
    /// A public write with a create-once keyed on a unique field (a
    /// newsletter email). The answer is the same whether the row is new or
    /// already there, and names no id (owner, 2026-10-12, ask 31).
    WriteKeyed,
}

impl PublicMode {
    /// Whether this is a public write, keyed or not.
    pub fn is_write(self) -> bool {
        matches!(self, PublicMode::Write | PublicMode::WriteKeyed)
    }
}

/// The principal a tokenless call to a public op runs as. The `$` is the
/// character the platform reserves for names no caller can produce, so no
/// directory entry, members row or token can carry it
/// (`design-public-path.md` §1).
pub const ANONYMOUS: &str = "$anonymous";

/// Why a spec may not be served. Registration-time, never request-time.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpecError {
    /// A declared parameter whose name collides with the request envelope's
    /// closed `$`-key set (SH2-1). `$space` in particular must be
    /// unshadowable: the door binds it and the executor is handed it, so an
    /// op parameter of that name would be a second, caller-controlled space
    /// travelling in the parameter map.
    ReservedParamName { op: String, param: String },
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::ReservedParamName { op, param } => write!(
                f,
                "op `{op}` declares a parameter named `{param}` — `$`-prefixed names are the \
                 request envelope's ($refine, $page, $space) and cannot be op parameters"
            ),
        }
    }
}

impl std::error::Error for SpecError {}

impl OpSpec {
    /// Does this op's compiled write mode **create** rows?
    ///
    /// Read off [`OpSpec::mode_label`], the compiler's static write-mode word
    /// (`creates`, `converge-or-create`, `converges`, `deletes`). It decides
    /// one thing: whether an `Idempotency-Key` stays bound to its payload
    /// after the stored response expires (Z-018). A creating form carries no
    /// `rev`, so a replay with an edited payload has no CAS to stop it from
    /// silently converging onto the row the first call created — the binding
    /// has to outlive the cache, and only creating ops need it to.
    pub fn creates_rows(&self) -> bool {
        self.mode_label
            .as_deref()
            .is_some_and(|mode| mode.contains("create"))
    }

    /// Does this op's compiled write mode **delete** rows, in any of its
    /// statements?
    ///
    /// Wider than `kind == Delete`, which the host answers only for an op
    /// whose every statement is a `del`. An op that sets one row and
    /// deletes another is a `Write` whose label carries `deletes`, and a
    /// rule about deleting (SH-20: deletes are closed to delegated tokens)
    /// has to reach it too (audit 2026-10-08).
    pub fn deletes_rows(&self) -> bool {
        self.kind == OpKind::Delete
            || self
                .mode_label
                .as_deref()
                .is_some_and(|mode| mode.contains("delete"))
    }

    /// Structural check applied when a registry accepts a spec.
    pub fn validate(&self) -> Result<(), SpecError> {
        for param in &self.params {
            if param.name.starts_with('$') {
                return Err(SpecError::ReservedParamName {
                    op: self.name.clone(),
                    param: param.name.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Op lookup — fulfilled by the artifact registry later; a map in tests.
/// Returns an owned spec so callers hold their version across swaps.
pub trait OpRegistry: Send + Sync {
    fn get(&self, name: &str) -> Option<OpSpec>;
}

/// Parameters that passed type + bounds validation, keyed by declared name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundParams(pub serde_json::Map<String, serde_json::Value>);

#[derive(Clone, Debug, PartialEq)]
pub struct ExecResult {
    pub rows: Vec<serde_json::Value>,
    pub next_cursor: Option<PageCursor>,
    /// The space's command-log position the rows were read at, when the
    /// executor read them in one snapshot with it. The door pins the signed
    /// head at this position rather than whatever the head is by the time it
    /// answers. `None` for writes and for any result without a read position,
    /// which pins the current head as before.
    pub position: Option<u64>,
}

/// The executor's typed rejection — passed through the error envelope
/// untranslated, so the platform's rejections stay typed on the wire.
///
/// # Why this carries an [`ErrorCode`] rather than being a second enum
///
/// It *was* a second enum: eight variants, each restating an `ErrorCode`'s
/// name and most of its doc comment. Two closed inventories of the same
/// thing drift, and this pair had — the write protocol raises seventeen
/// typed refusals (`writing-data.md` §7 and the tree rules) and every one
/// past the eight arrived on the wire as `internal`, 500. A rule that is
/// *built* was indistinguishable from a server bug, which is the whole of
/// checklist 9g.
///
/// There is one inventory now. An executor refusal whose class has no wire
/// code cannot be constructed, and [`ErrorCode::executor_raisable`] keeps
/// the door's own words — `access_denied` above all — out of the
/// executor's mouth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecError {
    pub code: crate::error::ErrorCode,
    pub message: String,
    /// An HTTP status that replaces the class's own on the wire. Only
    /// `space_paused` honours it, and only for 4xx and 503: the pause
    /// answers the app developer set (2026-10-06). The class does not change.
    pub status: Option<u16>,
}

impl ExecError {
    pub fn new(code: crate::error::ErrorCode, message: impl Into<String>) -> Self {
        ExecError {
            code,
            message: message.into(),
            status: None,
        }
    }

    /// The executor's own failures — a bug, a disagreement it must not
    /// paper over, a tier that could not answer. 500.
    pub fn internal(message: impl Into<String>) -> Self {
        ExecError::new(crate::error::ErrorCode::Internal, message)
    }
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ExecError {}

impl ExecError {
    /// The `server_busy` answer: every connection was in use and nothing
    /// ran. The door adds `Retry-After`.
    pub fn server_busy() -> Self {
        ExecError::new(
            crate::error::ErrorCode::ServerBusy,
            crate::error::server_busy_message(),
        )
    }
}

impl From<ordo_identity::StoreFault> for ExecError {
    /// A store that did not answer, by its typed class: busy is
    /// `server_busy` (retried by every client), anything else is
    /// `space_unavailable` with the store's own sentence.
    fn from(f: ordo_identity::StoreFault) -> Self {
        if f.is_busy() {
            ExecError::server_busy()
        } else {
            ExecError::new(crate::error::ErrorCode::SpaceUnavailable, f.reason())
        }
    }
}

/// A reservation the executor is expected to finish **inside the same
/// transaction as the effect**.
///
/// # Why this rides into the executor (AA-004's third leg)
///
/// The router reserves an idempotency key before executing and stores the
/// outcome after. Between those two moments the effect commits. A process
/// that dies in that window leaves a claimed key with a committed effect
/// and no recorded outcome — and when the reservation ages out, the retry
/// executes the write a second time. Storing the outcome *earlier* only
/// inverts the failure: then a crash leaves a recorded outcome with no
/// effect.
///
/// There is no ordering of two independent writes that fixes this, so the
/// lease is handed to the layer that owns the transaction. An executor
/// that receives one must record the key's completion durably in the same
/// unit of work as the effect, and must answer [`ExecOutcome::AlreadyApplied`]
/// when it finds the key already recorded. The in-memory response cache
/// stays a verbatim-replay convenience on top of that record; it is no
/// longer what makes the guarantee.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayLease {
    /// Space, principal, op, and the caller's key — the full identity, so
    /// the journal cannot collide across principals or spaces.
    pub key: String,
    /// The request fingerprint, so a durable record can refuse a replay
    /// whose payload changed.
    pub fingerprint: String,
}

/// What an execution did. `AlreadyApplied` is not an error: it is the
/// truthful answer when a lease's effect committed under an earlier
/// attempt that died before the response was stored.
#[derive(Clone, Debug, PartialEq)]
pub enum ExecOutcome {
    Ran(ExecResult),
    AlreadyApplied,
}

/// Runs a prepared plan **in a named space**. Boxed future rather than
/// `async fn` so the trait stays object-safe behind `Arc<dyn OpExecutor>`.
///
/// # Why `space` is a parameter and not part of `BoundParams` (SH2-1 / Y-015)
///
/// The door binds the space and the access check is made against it. If the
/// executor does not receive it, it has to get one from somewhere, and every
/// candidate is a break: from the caller's parameters (approve one space,
/// execute against another), from the token (which collapses the multi-space
/// product `$space` exists to serve), or from ambient state (at which point
/// isolation is decorative).
///
/// So it rides as its own named argument, where an op parameter cannot shadow
/// it — `$space` is stripped from the parameter map by the router and
/// rejected as a declared parameter name at registration
/// ([`OpSpec::validate`]).
///
/// Implementations **assert** that the space they plan against is this one. A
/// disagreement is [`ExecError::Internal`], never a silent choice.
pub trait OpExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: BoundParams,
        refine: Option<Refinement>,
        page: Option<PageCursor>,
        // `lease` is present only for a write carrying an idempotency key.
        // An executor that ignores it is correct but not crash-safe; one
        // that honours it closes AA-004's third leg (see `ReplayLease`).
        lease: Option<ReplayLease>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecError>>;

    /// [`OpExecutor::execute`] for a read whose caller sets the page size.
    ///
    /// A sync feed is a background load with its own page size
    /// (`ORDO_SYNC_PAGE_ROWS`); the interactive read keeps the executor's
    /// own. An executor that pages implements this and has `execute` call
    /// it with `None`. The default ignores `page_rows`, which is right for
    /// an executor that does not page.
    #[allow(clippy::too_many_arguments)]
    fn execute_sized<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: BoundParams,
        refine: Option<Refinement>,
        page: Option<PageCursor>,
        lease: Option<ReplayLease>,
        page_rows: Option<usize>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecError>> {
        let _ = page_rows;
        self.execute(op, space, bound, refine, page, lease)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessDecision {
    /// The door opened, and it names the principal it resolved. The token is
    /// interpreted in exactly one place — here — so everything downstream
    /// that needs attribution (the idempotency cache's key, the command log's
    /// actor) takes it from the decision rather than re-parsing a bearer
    /// string. Without it the replay cache cannot be principal-scoped, which
    /// is the whole of SH-17.
    Allow {
        /// Who `$caller` is: the member the row guards evaluate against,
        /// and the principal the replay cache and the page cursor key on.
        /// Under a delegation this is the **delegator**
        /// (`plan-delegation-surface.md` §7, increment 2): the door has
        /// already bounded the call by the delegator's reach, and the row
        /// layer answers for the same person.
        principal: String,
        /// The principal presenting the token when it acts for `principal`
        /// under a live delegation, and `None` otherwise. It never reaches a
        /// guard. It reaches the command log, so a delegated write is
        /// recorded as `agent:<agent> on behalf of user:<principal>`.
        agent: Option<String>,
    },
    Deny,
}

impl AccessDecision {
    /// An ordinary allow: the token's holder acts as itself.
    pub fn allow(principal: impl Into<String>) -> Self {
        AccessDecision::Allow {
            principal: principal.into(),
            agent: None,
        }
    }
}

/// One check at the door: token + **space** + op → may this principal call
/// this op *in this space*? Runs before binding, planning, or any executor
/// work.
///
/// The space parameter is not optional and not derivable here (SH-19 /
/// X-040): a human belongs to many spaces, so the space comes from the
/// request envelope, and an access model that cannot see it cannot stop a
/// member of space A from calling a registered op against space B's data.
/// This mirrors `ordo-identity::AccessEngine::check(claims, space, op, now)`,
/// which has been space-scoped all along.
///
/// Implementations answer `Deny` for a space that does not exist or that the
/// principal has no membership in — never a distinct "no such space", which
/// would enumerate the deployment's spaces.
#[async_trait::async_trait]
pub trait AccessChecker: Send + Sync {
    fn check(&self, token: &str, space: &str, op: &str) -> AccessDecision;

    /// The **token gate**: what a bearer string is refused for before any op
    /// name or space is looked at.
    ///
    /// # Why there is a second, earlier check at all
    ///
    /// [`AccessChecker::check`] is deliberately synchronous and pure — the
    /// engine behind it decides from the token and in-memory access state,
    /// with no I/O, so a registry outage cannot become an authorization
    /// outage. Exactly one refusal cannot be decided that way: whether a
    /// **delegation** has been withdrawn. A bearer string cannot express
    /// "revoked", and DL §8 calls delegation revocable, so the answer lives
    /// in a registry and the door has to ask.
    ///
    /// Making it its own step rather than widening `check` buys two things:
    ///
    /// - **It runs before the op registry is consulted**, which is what
    ///   keeps [`crate::ErrorCode::DelegationRequired`] from being an
    ///   op-existence oracle. If it ran inside `check`, an unregistered name
    ///   (answered `access_denied` without ever reaching the checker) and a
    ///   registered one would give a dead-delegation caller two different
    ///   codes — and the difference between them would be a map of the
    ///   deployment's op surface.
    /// - **Nothing else pays for it.** The default is [`TokenGate::Pass`], so
    ///   an implementation with no delegations to check does no work and
    ///   needs no code.
    ///
    /// It answers about the token and nothing else. Anything that depends on
    /// which op, which space, or which principal belongs in `check`.
    async fn token_gate(&self, _token: &str) -> TokenGate {
        TokenGate::Pass
    }

    /// Whether `token` is one this deployment issued: it verifies, whatever
    /// it may or may not be allowed to do.
    ///
    /// Asked by `GET /live` (27s), which opens a stream that holds a
    /// connection before any op or space is named, so [`Self::check`]
    /// cannot run there. Without this a bearer string nobody signed would
    /// hold a stream for the idle window, as many times as it liked. It
    /// decides nothing about access: every read added to the stream still
    /// meets `check`. The default says yes, for an embedding whose tokens
    /// are its own business.
    fn token_known(&self, _token: &str) -> bool {
        true
    }

    /// The space a tokenless call to a **public** op runs in: the
    /// deployment's own space (`design-public-path.md` §1). `None`, the
    /// default, opens no anonymous door at all, so an embedding that does not
    /// say otherwise answers every tokenless call `access_denied` as before.
    fn anonymous_space(&self) -> Option<String> {
        None
    }

    /// What kind of caller `token` is, for the one door that treats them
    /// differently: a person's own download. The default is
    /// [`CallerKind::Person`], so an embedding that has no delegations or
    /// share grants changes nothing.
    fn caller_kind(&self, _token: &str) -> CallerKind {
        CallerKind::Person
    }

    /// Asked only after [`Self::check`] denied: is `space` a tenant still
    /// being set up for the person `token` proves? The answer is the
    /// `Retry-After` wait in seconds, and the door then refuses with
    /// [`crate::ErrorCode::TenantNotReady`] instead of `access_denied`.
    ///
    /// It answers only for that person. A caller with no claim to the
    /// tenant keeps the door's uniform denial. The default is `None`, so an
    /// embedding with no tenants being set up changes nothing.
    fn not_ready(&self, _token: &str, _space: &str) -> Option<u64> {
        None
    }

    /// Asked only after [`Self::check`] denied: is the door's member list
    /// for `space` behind its database? True when the last rebuild could
    /// not read that space's members, or a rebuild for a change in it
    /// failed. The door then answers [`crate::ErrorCode::ServerBusy`], a
    /// retryable 503 with `Retry-After`, in place of `access_denied`: it
    /// cannot tell a non-member from a member it has not read yet, and a
    /// denial would tell a member their permission is gone (reported fixes
    /// tenant-access-lags-after-join and pool-exhaustion-reads-as-denied).
    ///
    /// It answers only for a verified, live token. A caller with no valid
    /// token keeps the uniform denial. The default is `false`.
    fn behind(&self, _token: &str, _space: &str) -> bool {
        false
    }

    /// Asked only after [`Self::check`] denied: would this caller be
    /// allowed `op` in `space` with a fresher passkey touch, and with
    /// nothing else changed? Then the door answers
    /// [`crate::ErrorCode::PresenceRequired`] in place of its uniform
    /// denial (`plan-identity-service.md` 4.3). Only a deployment token
    /// answers yes: its holder is already known to the door, so the answer
    /// says nothing a stranger could use. The default is `false`.
    fn presence_required(&self, _token: &str, _space: &str, _op: &str) -> bool {
        false
    }

    /// Asked only after [`Self::check`] allowed `principal` to call `op`:
    /// does the app developer require a verified email address for this
    /// op, and does this person lack one? The answer is the refusal's
    /// message, which names the setting, and the door then refuses with
    /// [`crate::ErrorCode::EmailRequired`].
    ///
    /// The default is `None`, so an embedding that requires nothing
    /// changes nothing.
    ///
    /// When the table that holds addresses cannot be read, the answer is an
    /// [`EmailGate`] with `retryable` set: the call is refused, and the same
    /// call lands once the table is back (owner, 2026-10-22).
    async fn email_required(&self, _token: &str, _principal: &str, _op: &str) -> Option<EmailGate> {
        None
    }

    /// Record a refusal the door made **without** asking [`Self::check`]:
    /// no token, an op nobody registered, a download that is off or closed
    /// to this caller. `check` records its own denials; these return before
    /// it, so without this the security stream never saw them.
    ///
    /// The record goes to the space's stream when `token` proves a
    /// principal with a relationship to `space`, and to the deployment's
    /// otherwise; the stream's collapse window bounds its volume, as it
    /// does for `check`'s. The door calls it only through
    /// `router::door_refusal`, the one recording path. The default records
    /// nothing, for an embedding with no security stream.
    fn record_refusal(
        &self,
        _token: Option<&str>,
        _space: Option<&str>,
        _op: &str,
        _why: DoorRefusal,
    ) {
    }
}

/// Why the door refused a call that [`AccessChecker::check`] never saw. The
/// wire answer is the door's uniform `access_denied` (or the download's own
/// sentence); this is what the security record says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorRefusal {
    /// No bearer token, and not a call the anonymous door serves.
    TokenMissing,
    /// The op name is not registered.
    UnregisteredOp,
    /// A person's own download, with no op marked for it: it is off.
    DownloadOff,
    /// A person's own download, with a token that is not the person's own.
    DownloadNotForCaller,
}

/// Who holds a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallerKind {
    /// The person, signed in.
    Person,
    /// A token that acts for a person (a worker or an agent).
    Delegated,
    /// A share grant's holder: a guest of somebody else's data.
    Guest,
}

/// Why the door holds back a call for want of a verified email address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailGate {
    /// Names the setting that requires the address.
    pub message: String,
    /// `true` when the address table could not be read, so a later try may
    /// pass. `false` when the person has no address.
    pub retryable: bool,
}

impl EmailGate {
    /// The person has no verified address.
    pub fn missing(message: impl Into<String>) -> Self {
        EmailGate {
            message: message.into(),
            retryable: false,
        }
    }

    /// The table could not be read; try again.
    pub fn unreadable(message: impl Into<String>) -> Self {
        EmailGate {
            message: message.into(),
            retryable: true,
        }
    }
}

/// What the token gate decided. Not an [`AccessDecision`]: it names no
/// principal, because it runs before anything has been authorized, and its
/// refusal has its own wire class rather than the door's uniform one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenGate {
    /// Nothing at this layer refuses the token. Says nothing about whether
    /// any op is callable — that is [`AccessChecker::check`]'s answer.
    Pass,
    /// The registry that says whether the token's share grant or delegation
    /// is live could not be read. Answered
    /// [`crate::ErrorCode::ServerBusy`], a retryable 503: the door does not
    /// know, so it neither admits nor denies (reported fix
    /// pool-exhaustion-reads-as-denied).
    Busy,
    /// The token claims a delegation that is not live. Answered
    /// [`crate::ErrorCode::DelegationRequired`] (403) — see that variant for
    /// why this one refusal is allowed to be distinguishable when the door's
    /// others are not.
    DelegationNotLive,
    /// The token holds a share grant that is no longer live. Answered as the
    /// door's uniform `access_denied`, before the op registry is consulted,
    /// so a closed window tells its holder nothing an open one would not.
    ShareNotLive,
}

// ── quotas (17n M2) ────────────────────────────────────────────────────────

/// What a space's quota for one quantity says about the request in front of
/// it. `plan-production-ops.md` M2: a quota has a limit and a grace — over
/// the limit warns and continues, over limit **plus** grace refuses. A
/// hosted plan that hard-stops at the boundary with no grace is a product
/// decision a deployment makes by setting `grace` to zero, not one this
/// enum makes for every deployment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaDecision {
    /// Nothing configured, or configured and not reached.
    Within,
    /// Over the limit, under limit + grace: served, and recorded so the
    /// deployment surface can show it.
    Warn {
        quantity: &'static str,
        used: u64,
        limit: u64,
    },
    /// Over limit + grace: refused. [`crate::ErrorCode::QuotaExceeded`]
    /// (429), raised in [`crate::router::gate`] **after**
    /// [`AccessChecker::check`] and never before — a quota refusal handed to
    /// a non-member would disclose that the space exists and is busy, where
    /// `access_denied` discloses nothing (`corpus-rulings.md`, `write-quota`,
    /// 2026-09-19).
    Exceeded {
        quantity: &'static str,
        used: u64,
        limit: u64,
        grace: u64,
    },
}

/// One check at the door, after access: may this space still call, given
/// what it has already used? `crate::router::gate` runs it immediately
/// after [`AccessChecker::check`] and nowhere else.
///
/// Synchronous and pure, the same shape as [`AccessChecker::check`] and for
/// the same reason: the engine behind it reads a snapshot a worker
/// refreshes on a tick, with no I/O on the request path, so a database
/// outage cannot become a door outage.
///
/// **v1 gates `requests` and `rows_returned` only.** Both are counted at
/// the door as they happen, so the quota is exact. `writes` is counted from
/// the command log at rollup and would bite up to a rollup period late,
/// which is a quota in name only — deferred to 18i
/// (`corpus-rulings.md`, `write-quota`, 2026-09-19).
pub trait QuotaGate: Send + Sync {
    fn check(&self, space: &str, op: &OpSpec) -> QuotaDecision;
}

/// The default gate: every space is always [`QuotaDecision::Within`]. What
/// every `ApiState` before M2 effectively had, made explicit rather than
/// left as a `None` some call sites forget to handle — `ApiState.quota`
/// takes this when metering, and therefore quotas, are off.
pub struct NoQuota;

impl QuotaGate for NoQuota {
    fn check(&self, _space: &str, _op: &OpSpec) -> QuotaDecision {
        QuotaDecision::Within
    }
}

/// A staleness signal, never a data push — the client refetches through the
/// normal op under its own scope (refetch-through-scope semantics).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidationEvent {
    pub op: String,
}

/// Per-op invalidation fan-out — fulfilled by the command-log listener later.
pub trait InvalidationBus: Send + Sync {
    fn subscribe(&self, op_name: &str) -> broadcast::Receiver<InvalidationEvent>;

    /// The entities whose writes invalidate `op_name`, as the bus indexes
    /// them. The public cache tags its answers with them for a CDN
    /// (`Cache-Tag: entity:<name>`). The default knows none.
    fn entities_of(&self, _op_name: &str) -> Vec<String> {
        Vec::new()
    }

    /// Subscribe for a feed on `space`, and hold whatever the bus needs open
    /// for as long as the returned [`Subscription`] lives.
    ///
    /// A bus that hears a space's writes over a connection of its own (the
    /// Postgres bus, one per database) opens it here and lets it go when the
    /// last feed on that database has ended, so a tenant nobody is watching
    /// holds no listening connection. The default holds nothing.
    fn subscribe_in<'a>(&'a self, op_name: &'a str, _space: &'a str) -> Subscribing<'a> {
        Box::pin(async move {
            Subscription {
                receiver: self.subscribe(op_name),
                lease: None,
                hearing: None,
            }
        })
    }
}

/// What [`InvalidationBus::subscribe_in`] returns.
pub type Subscribing<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Subscription> + Send + 'a>>;

/// A feed's subscription: the signals, and the lease that keeps the bus
/// listening for them. The feed keeps the whole value for as long as it is
/// open, because dropping the lease is what tells the bus it may stop.
pub struct Subscription {
    pub receiver: broadcast::Receiver<InvalidationEvent>,
    pub lease: Option<Box<dyn std::any::Any + Send + Sync>>,
    /// Whether the bus is hearing the database this subscription is on, and
    /// how many times it has stopped hearing it. `None` for a bus that cannot
    /// lose its source (the in-memory bus). The public cache watches it: an
    /// entry stored before a lost listener could have missed a write, so a
    /// loss drops every entry, and nothing is stored while deaf
    /// (`design-public-path.md` §3, "A lost listener").
    pub hearing: Option<tokio::sync::watch::Receiver<Hearing>>,
}

/// What [`Subscription::hearing`] carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hearing {
    /// The listener is connected now.
    pub heard: bool,
    /// How many times the listener has been lost. A change means writes may
    /// have gone unheard.
    pub losses: u64,
}
