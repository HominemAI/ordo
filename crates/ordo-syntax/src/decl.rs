//! Declaration and pipeline AST (spec §2 shapes, §5 pipelines).
//!
//! `ast.rs` stops at the closed expression/condition grammar — the part that
//! is reusable everywhere. This module is the layer above it: what a `domain`
//! file actually contains (entities, ops) and the pipeline shape an op's body
//! compiles from. Every node carries a span for the same reason `ast.rs` does:
//! the binder and the planner always have a place to point at.
//!
//! Scope note for readers: the parser here accepts the whole fixture library
//! of `composition-conformance.md` §2.3 — including `group`, `take`, and
//! parameter defaults — because the corpus registers that library *before*
//! any case runs. What the planner can currently lower is a strictly smaller
//! set; unsupported constructs are rejected downstream by name, never here.

use crate::ast::{Condition, Expr, Ident};
use crate::span::Span;
use crate::token::DurationUnit;
use crate::trigger::{ListenerPattern, Modifiers, Schedule};

/// A parsed source file: at most one `domain` header, then declarations.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Program {
    pub domain: Option<DomainDecl>,
    pub entities: Vec<EntityDecl>,
    pub ops: Vec<OpDecl>,
    /// `release member …`: whether a member may leave the tenant on their
    /// own, and what happens to the rows that point at them (checklist
    /// 27aq). At most one per program. Absent means **locked**: nobody
    /// leaves on their own (owner ruling 2026-10-05, `person-can-leave`
    /// ask 4).
    pub release: Option<ReleaseDecl>,
    /// `routine name { trigger … run: … }`: scheduled and on-change work the
    /// platform runs in every tenant (DL §7, §8; checklist 12a).
    pub routines: Vec<RoutineDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainDecl {
    pub name: Ident,
    /// `@ 1.0.0` — kept as written; version policy is not the parser's.
    pub version: Option<String>,
    pub span: Span,
}

// ── shapes ───────────────────────────────────────────────────────────────

/// The closed scalar type set (spec §2). `money` is whole minor units,
/// integral by construction, and carries its currency's places;
/// `duration` carries its declared unit.
///
/// # Places on `money` and scale on `decimal` (ruled 2026-10-04, 27ak)
///
/// `money(2)` stores whole cents and is written in major units: the
/// source says `29.99` and the compiler converts it to `2999`. The places
/// are part of the type, so a `money(2)` value and a `money(0)` value do
/// not mix without the author saying how (`money_places_mismatch`). Bare
/// `money` is `money(0)`: whole units, exactly what `money` meant before
/// places existed, so every program written before the ruling keeps both
/// compiling and meaning what it meant.
///
/// `Decimal(s)` is an exact decimal with **at most `s` digits after the
/// point**. As a field or parameter it is `decimal(s)`, stored as a
/// Postgres `numeric` with that scale. As an expression type the scale is
/// the most a value can carry: `/` and `avg` produce scale 12 (DL §2,
/// Division), `+`/`-` keep the larger scale, `*` adds them. The scale is
/// what lets the compiler say, without running anything, whether a value
/// fits a `decimal(s)` slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarTy {
    Text,
    Int,
    /// Whole minor units, with the currency's places.
    Money(u8),
    /// An exact decimal with at most this many places.
    Decimal(u8),
    Bool,
    Date,
    Datetime,
    Duration(DurationUnit),
    Id,
    /// Managed binary content (DL §2, Content types). The **row holds a
    /// reference**, never bytes: an opaque keyed-random handle whose
    /// object lives in the space's resolved store, or — with `inline` —
    /// whose bytes live in the sealed payload store.
    ///
    /// A blob value is **opaque**. It is not comparable, not sortable, not
    /// aggregatable except by `count`, never `unique` or `indexed`, never
    /// a parameter, never a default, and never the RHS of a write. Every
    /// one of those refusals is argued where it is enforced; the short
    /// version is that a keyed-random handle has no order and no
    /// meaningful equality, and a language that let you write `f == g`
    /// would be promising something the addressing scheme deliberately
    /// destroyed. `f.mime` and `f.size` are ordinary values and behave
    /// ordinarily.
    Blob,
    /// An external pointer with no custody (DL §2): an address and an
    /// optional integrity hash. `uri` is **text-shaped** — comparable,
    /// sortable, uniquable, sealable, blind-indexable — because it holds
    /// an address, not a handle. Nothing is served, nothing is
    /// garbage-collected, and no bytes are exported; the consent page says
    /// "referenced, not held".
    Uri,
    /// A JSON document (checklist 28b): any JSON value, stored as Postgres
    /// `jsonb` and carried on the wire as JSON itself, never as a string.
    ///
    /// A whole `json` value is **opaque to the language**: it is written
    /// whole and read whole, never compared, sorted, aggregated, `unique` or
    /// `index`ed (`json_opaque`). A condition may walk into one with a path
    /// (`prefs.theme == "dark"`), typed by the condition's other side.
    /// Inside the compiler and the write protocol it travels as its
    /// compact JSON text, which is what lets a sealed `json` be stored
    /// exactly as a sealed `text` is.
    Json,
}

impl ScalarTy {
    /// Every member of the closed set, with `Duration(Ms)` standing for
    /// the duration family.
    ///
    /// # Why this exists, and what it closed
    ///
    /// `crates/ordo-syntax/tests/editor_grammar.rs` cross-checks the
    /// editor grammar against the lexer for reserved words, event kinds,
    /// expression functions, aggregates, allocate modes and duration
    /// units — six inventories, each held by an `ALL` on its producer. It
    /// did **not** check `support.type.ordo`, because no `ScalarTy::ALL`
    /// existed. So adding a type to the language and forgetting the
    /// grammar left it uncoloured, and every editor user learned the type
    /// set was one word shorter than it is, with no test failing.
    ///
    /// That gap pre-dates the `blob`/`uri` change and is closed by it.
    /// [`ScalarTy::DECLARABLE`] is the half a *field* may be declared as;
    /// the grammar is asserted against that, because `decimal` is an
    /// expression type with no declaration form and colouring it in a
    /// type position would teach the wrong thing.
    ///
    /// The list is written out rather than derived because `Duration`
    /// carries a unit, so there is no `Self::VARIANTS` a macro could
    /// produce. `all_is_total` below is what keeps it honest: it is an
    /// exhaustive match, so a new variant breaks the build there rather
    /// than being quietly missing here.
    pub const ALL: &'static [ScalarTy] = &[
        ScalarTy::Text,
        ScalarTy::Int,
        ScalarTy::Money(0),
        ScalarTy::Decimal(DIVISION_SCALE),
        ScalarTy::Bool,
        ScalarTy::Date,
        ScalarTy::Datetime,
        ScalarTy::Duration(DurationUnit::Ms),
        ScalarTy::Id,
        ScalarTy::Blob,
        ScalarTy::Uri,
        ScalarTy::Json,
    ];

    /// The members a field may be **declared** as: every member of
    /// [`ScalarTy::ALL`]. `decimal` joined on 2026-10-04 (27ak); it was an
    /// expression type only until the owner ruled a storable `decimal(p)`
    /// for rates and unit prices.
    ///
    /// This is the list the type keyword table admits, the list its
    /// closed-set rejection message names, and the list the editor grammar
    /// is asserted against.
    pub const DECLARABLE: &'static [ScalarTy] = &[
        ScalarTy::Text,
        ScalarTy::Int,
        ScalarTy::Money(0),
        ScalarTy::Decimal(DIVISION_SCALE),
        ScalarTy::Bool,
        ScalarTy::Date,
        ScalarTy::Datetime,
        ScalarTy::Duration(DurationUnit::Ms),
        ScalarTy::Id,
        ScalarTy::Blob,
        ScalarTy::Uri,
        ScalarTy::Json,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ScalarTy::Text => "text",
            ScalarTy::Int => "int",
            ScalarTy::Money(_) => "money",
            ScalarTy::Decimal(_) => "decimal",
            ScalarTy::Bool => "bool",
            ScalarTy::Date => "date",
            ScalarTy::Datetime => "datetime",
            ScalarTy::Duration(_) => "duration",
            ScalarTy::Id => "id",
            ScalarTy::Blob => "blob",
            ScalarTy::Uri => "uri",
            ScalarTy::Json => "json",
        }
    }

    /// Does this type hold **content** — bytes the platform stores and
    /// serves — rather than a value?
    ///
    /// The one predicate the fourteen refusal sites share, so they all say
    /// the same thing rather than each spelling `matches!(t, Blob)`.
    /// `uri` answers `false`: it is an address, and an address is a value.
    pub fn is_content(self) -> bool {
        matches!(self, ScalarTy::Blob)
    }

    /// The places a `money` carries or the scale a `decimal` carries;
    /// `None` for every other type.
    ///
    /// The artifact, the interface descriptor and the door read this, so
    /// a client knows how to spell `2999` minor units as `"29.99"`.
    pub fn places(self) -> Option<u8> {
        match self {
            ScalarTy::Money(p) | ScalarTy::Decimal(p) => Some(p),
            _ => None,
        }
    }

    /// The type as source spells it: `money(2)`, `decimal(4)`,
    /// `duration(ms)`, and the bare word for the rest. Bare `money` is
    /// `money(0)` and is spelled bare, so a program written before places
    /// existed reads back exactly as it was written.
    pub fn spelled(self) -> String {
        match self {
            ScalarTy::Money(0) => "money".to_string(),
            ScalarTy::Money(p) => format!("money({p})"),
            ScalarTy::Decimal(p) => format!("decimal({p})"),
            ScalarTy::Duration(u) => format!("duration({})", u.as_str()),
            other => other.as_str().to_string(),
        }
    }
}

/// The fixed fractional scale of `/` and `avg` (DL §2, Division: "twelve
/// digits after the decimal point"). It is also the most places a
/// `money(p)` or a `decimal(p)` may declare: no stored value is finer
/// than the language computes, so a stored value widened into an
/// expression never needs rounding.
pub const DIVISION_SCALE: u8 = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Scalar(ScalarTy),
    /// `parent -> categories?  tree` — a many→one edge. `owned` marks the
    /// ownership edge (one per entity, spec §2); `tree` marks a self-edge
    /// that carries the implicit `depth`.
    Ref {
        target: Ident,
        owned: bool,
        tree: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: Ident,
    pub kind: FieldKind,
    /// `?` — the field admits null.
    pub optional: bool,
    /// `title: text(64)` — the declared length bound of a `text` field,
    /// in characters (DL §2, Types: "`text` (boundable `text(64)`)").
    ///
    /// Carried beside the type rather than inside [`ScalarTy::Text`]
    /// because the bound is a constraint on the field, not a different
    /// type: a `text(64)` value flows everywhere a `text` value does, and
    /// every consumer that matches `ScalarTy::Text` keeps matching. The
    /// schema lowers it to a `CHECK (char_length(...) <= n)` on an open
    /// column and the write path refuses an over-long value before any
    /// SQL runs, which is what covers a sealed field too. `None` for every
    /// other type and for an unbounded `text`.
    pub text_max: Option<u32>,
    pub default: Option<Expr>,
    pub unique: bool,
    /// `index` on an open field: a plain single-column index, the one-field
    /// form of the entity clause `index (a, b)` (reported fix 25w). Lowered
    /// into the entity's `indexes`; refused on a sealed field, whose index
    /// is the blind index `indexed`.
    pub index: bool,
    pub sealed: bool,
    /// `sealed indexed` — contributes a blind-index digest column.
    pub indexed: bool,
    /// `sealed indexed refinable` — opens `eq`/`in` refinement.
    pub refinable: bool,
    /// `blob private` — **client-sealed custody** (DL §2, Content types):
    /// the client encrypts before upload with a passkey-PRF-derived key,
    /// the platform stores ciphertext and holds no key, and no server-side
    /// processing is possible by design.
    ///
    /// Parsed, bound and carried into the schema; **refused at every
    /// runtime path that cannot serve it**, because F7 Phase 6 owns the
    /// passkey-PRF derivation, the device wrapping and the presigned
    /// direct-to-bucket upload. Admitting the word and refusing the act is
    /// deliberate: a language that cannot spell the tier cannot refuse it
    /// by name, and "unknown modifier `private`" is a worse answer than
    /// "this is client-sealed custody and the key ceremony has not landed".
    pub private: bool,
    /// `blob inline` — the bytes live in the sealed payload store rather
    /// than in object storage (DL §2, pin 2).
    ///
    /// A **storage-tier** modifier, orthogonal to the custody modifiers,
    /// exactly as DL says: *"the three custody tiers (`blob` /
    /// `blob private` / `uri`) compose with `inline` orthogonally."* It is
    /// contextual rather than reserved, for `refinable`'s reason: reserving
    /// the word would break every existing schema with a field named
    /// `inline`, and the modifier position is the only place it can appear.
    pub inline: bool,
    /// `normalize(lower)` and friends.
    pub normalize: Option<Ident>,
    /// `price: money public` — a public op may serve this field to a
    /// visitor who is not signed in (`plan-syntax-additions.md` §1). Only
    /// what was written here; the entity line's `public` and an owner's
    /// `public*` are resolved by the schema.
    pub public: bool,
    pub span: Span,
}

/// The entity line `public` or `public*` (`plan-syntax-additions.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicMark {
    /// `public*`: the mark also reaches every entity this one owns,
    /// transitively, along `owned` refs (the path G6 inherits guards along).
    pub cascade: bool,
    pub span: Span,
}

/// The closed guard-walk grammar (DL G1): disjuncts of conjuncts of atoms.
/// No functions, no arithmetic, no `!=`, no ordering, no `in` — violations
/// are the binder's `invalid_guard_walk`, not a parse error, so the message
/// can name the walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardAtom {
    /// `path == $caller` or `path == "literal"`.
    Cmp {
        path: Vec<Ident>,
        caller: bool,
        lit: Option<Expr>,
        span: Span,
    },
    /// A bare bool field (or path to one).
    Bare { path: Vec<Ident>, span: Span },
    /// A reverse collection step: `projectMembers(member == $caller)`,
    /// optionally behind forward hops (`project.projectMembers(…)`) and
    /// optionally naming the arriving ref (`groupMembers.group(…)`).
    Step {
        path: Vec<Ident>,
        inner: Box<GuardExpr>,
        span: Span,
    },
}

/// `or` of `and`s, parens-when-mixing enforced by the parser.
///
/// **This is disjunctive normal form, always** — a parenthesized group
/// (R-G3 admits them when mixing `and`/`or`) is DISTRIBUTED by the parser
/// rather than kept as a nested node, so `(a or b) and c` arrives here as
/// `(a and c) or (b and c)`. The reason is the same one the addressed
/// delete gives for lowering `{ id: $id }` into the condition it means:
/// one representation is one behaviour, two could drift.
///
/// Distribution is sound because the guard grammar has **no `not`** —
/// there is no negation for De Morgan to flip — and it is what keeps every
/// downstream reading of "a disjunct" correct rather than approximately
/// correct. A disjunct is an *independent authorization path*, and three
/// separate rules are stated over exactly that notion: at least one
/// disjunct must reference `$caller` (G2), a write guard is create-exempt
/// only when EVERY disjunct contains a reverse step rooted at the new row
/// (DL §3, `corpus-rulings.md`), and the lowering unions one semi-join per
/// disjunct (R-G6). A nested node would have made all three read past the
/// parens and answer about a walk the author did not write.
///
/// The cost is that the manifest renders the normalized form, not the
/// author's factored one. That is the better disclosure: a reviewer of an
/// `access` clause is asking "what are the ways in?", and the normalized
/// form is that list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardExpr {
    pub disjuncts: Vec<Vec<GuardAtom>>,
    pub span: Span,
}

/// One `access <verb>` clause: a walk, or the reserved word `open` (G9 —
/// open-by-declaration, greppable, never inside a walk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardClause {
    Walk(GuardExpr),
    Open { span: Span },
}

// ── member release ───────────────────────────────────────────────────────

/// `release member open | locked | when <rule> [{ effects }]` (checklist
/// 27aq, `plan-member-release.md` §2).
///
/// The app developer's rule for when a member may leave a tenant on their
/// own, and what leaving does to the rows that point at the seat. Ordo
/// provides the mechanism and builds no override past it (owner ruling
/// 2026-10-05, ask 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseDecl {
    pub mode: ReleaseMode,
    /// What happens to rows that ref the seat. A ref no effect names keeps
    /// pointing at the seat, which reads as a former member (ask 6).
    pub effects: Vec<ReleaseEffect>,
    /// `release` through the end of the line or the closing brace.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseMode {
    /// Always true: any member may leave.
    Open { span: Span },
    /// Always false: nobody leaves on their own.
    Locked { span: Span },
    /// The conditions, all of which must hold for one of the disjuncts.
    When(ReleaseCond),
}

/// `or` of `and`s of release atoms, normalized exactly as a guard is.
///
/// Distribution stays sound with `not` because `not` binds to one atom
/// only: `not (a or b)` is refused by the parser, so no De Morgan is owed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseCond {
    pub disjuncts: Vec<Vec<ReleaseAtom>>,
    pub span: Span,
}

/// `[not] entity(inner)`: whether any row of `entity` matches `inner`.
///
/// `inner` is the guard grammar (G1). `$member` in it is the seat being
/// released, stored as the guard's subject (`GuardAtom::Cmp { caller:
/// true }`), and `$caller` is refused there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAtom {
    pub negated: bool,
    pub entity: Ident,
    pub inner: GuardExpr,
    pub span: Span,
}

/// One line inside a release block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseEffect {
    /// `remove tasks.assignee`: delete the rows whose ref names the seat.
    Remove {
        entity: Ident,
        field: Ident,
        span: Span,
    },
    /// `reassign tasks.assignee to people(member == $member).manager or owner`:
    /// point those rows at the first target that names an active member.
    Reassign {
        entity: Ident,
        field: Ident,
        to: Vec<ReassignTarget>,
        span: Span,
    },
}

/// Where a reassigned row's ref goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassignTarget {
    /// The tenant's longest-standing active owner other than the leaver.
    Owner { span: Span },
    /// `people(member == $member).manager`: the `field` of the one row of
    /// `entity` that `inner` matches.
    Lookup {
        entity: Ident,
        inner: GuardExpr,
        field: Ident,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDecl {
    pub name: Ident,
    /// `entity projects (key: proj) { … }` — the typed-id prefix override.
    /// Absent, the key derives from the name (4-char words, hyphenated,
    /// common-word abbreviations); collisions fail at compile time.
    pub key: Option<Ident>,
    pub fields: Vec<FieldDecl>,
    /// `access read <walk>` — G9: declaring one verb without the other is
    /// the schema's `incomplete_access_clause`, not a parse error.
    pub access_read: Option<GuardClause>,
    pub access_write: Option<GuardClause>,
    /// `acl (read, write, admin)` — levels in declaration order, which IS
    /// their ordering (read ⊂ write ⊂ admin).
    pub acl: Option<Vec<Ident>>,
    /// `unique (a, b)` — composite uniqueness claims, in declaration order,
    /// one entry per clause (DL §3 Refs, "Uniqueness [settled]"). The
    /// single-column form is a FIELD modifier (`email: text unique`) and
    /// lands on [`FieldDecl::unique`]; this is the composite spelling and
    /// only the composite spelling, so a one-column `unique (a)` is
    /// refused rather than silently becoming a second way to say the same
    /// thing.
    ///
    /// The space column leads every generated unique index without syntax
    /// (DL, "Uniqueness is always scoped to the space"), so it is not in
    /// this list and cannot be put there.
    pub unique_keys: Vec<Vec<Ident>>,
    /// `index (a, b)` — extra secondary indexes (DL §3 "Indexes
    /// [settled]": "Composite/extra indexes declared with `index (a, b)`").
    /// Ref fields index automatically, so this is the declaration for the
    /// ones the ref graph does not imply.
    pub indexes: Vec<Vec<Ident>>,
    /// `check used <= seats` — row invariants (DL §3, "Row invariants:
    /// `check` [settled]"), in declaration order.
    ///
    /// The parser owns the *shape* only: a `check` is a condition, and any
    /// condition the language can write parses here. Which conditions may
    /// become a database CHECK — row-local, immutable, over non-sealed
    /// fields — is the schema's question, answered against the graph, the
    /// way `unique (a, b)`'s column names are.
    pub checks: Vec<CheckDecl>,
    /// `std sealed` — DL §3, "Sealed fields and derived std ops [settled]":
    /// the one greppable line that opts the whole entity's **derived** std
    /// ops into the sealed-inclusive manifest. Absent, an entity holding
    /// any sealed field derives its std ops over the open skeleton only, so
    /// granting the std family cannot disclose a sealed value. Hand-written
    /// ops are untouched either way: they project what they project.
    pub std_sealed: bool,
    /// `public` or `public*` on its own line: every field of this entity is
    /// public, and with `*` every entity it owns too.
    pub public: Option<PublicMark>,
    pub span: Span,
}

/// One `check <condition>` clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckDecl {
    pub cond: Condition,
    /// The `check` keyword through the end of the condition — what a
    /// diagnostic points at.
    pub span: Span,
}

// ── operations ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDecl {
    pub name: Ident,
    pub ty: ScalarTy,
    /// `$n: int <= 200` — the declared upper bound. DL §2 Parameters:
    /// "Parameters are declared with type and bounds at operation
    /// registration"; "Bounds are enforced at the door, before planning."
    ///
    /// **`<=` and nothing else.** Every bound the spec writes is `<=`;
    /// `>=` and a range form are `critique-consolidated.md` X-077's
    /// *proposed* fix (`[S]`, unbuilt), not a settled spelling, and one
    /// spelling per concept means the unproposed ones must not parse.
    ///
    /// Carried as the literal the author wrote so the registration gate
    /// can type it, the artifact can print it, and the door can compare
    /// against it. The types that admit one are `int`, `money`, `date`
    /// and `duration(<exact unit>)` — see `param_bound` in the parser.
    pub max: Option<Expr>,
    /// `$payee: text(64)` — the declared length bound of a `text`
    /// parameter, in characters (DL §2 Parameters: `$payee: text(64)`).
    /// Enforced at the door before planning, like `max`
    /// (`api-engine-design.md` §5: "> n chars ⇒ `bound_violation`"). See
    /// [`FieldDecl::text_max`] for why it rides beside the type.
    pub text_max: Option<u32>,
    pub default: Option<Expr>,
    /// `$today: date filled` — the SDK-supplied marker (DL §2 Parameters,
    /// "`filled` [settled]"). The caller *may* supply the value and the
    /// generated client fills it when the caller does not. It is not a
    /// default (the value is not in the artifact) and not optional (the
    /// door still requires it); it marks the execution-relative values an
    /// op body may not compute. `filled` on a `reproducible` op is a
    /// compile error: filling from a clock is the nondeterminism the flag
    /// forbids.
    pub filled: bool,
    pub span: Span,
}

/// Registration flags (engine-plan G9). Reserved words, so an op named
/// `unbounded` is a parse error rather than a silent flagless op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpFlags {
    pub unbounded: bool,
    pub unguarded: bool,
    pub reproducible: bool,
    /// `op billing nopause = ...`: the op keeps answering while its tenant is
    /// paused (checklist 27ar, owner 2026-10-05). Every other op of a paused
    /// tenant is refused `space_paused`.
    pub nopause: bool,
    /// `op pricing public = ...`: the op runs on the public path. A visitor
    /// with no token may call it, in the deployment's space, and it reads
    /// public fields only (`design-public-path.md` §1, §2).
    pub public: bool,
    /// `op allNotes download = ...`: the op's rows go into a person's own
    /// download (`GET /download`, owner ruling 2026-10-22). The op says which
    /// entity, which fields and which rows; a blob field it answers brings
    /// the files. No op marked is the download turned off. Contextual, not
    /// reserved, so a field named `download` keeps working.
    pub download: bool,
}

/// One write statement: `set entity { field: value, … }` (create or
/// converge — the identity fields decide the mode), `set entity(cond)
/// { … }` (bulk converge), `del entity(cond)`, `del entity { id: $id }`,
/// `del entity { id: $id } subtree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteStmt {
    Set {
        /// `let x = set E { … }` — the name this statement's row takes for
        /// the *later* statements of the same group (DL §3, "Naming a
        /// freshly created row in the same op" [settled]; R-G19).
        ///
        /// An `Option` on `Set` rather than a variant of its own, for the
        /// reason [`WriteStmt::Del::subtree`] gives: everything that asks
        /// "is this a `set`?" — the op kind on the wire, the mode label,
        /// the entities-touched analysis, rules 2 and 3 — must keep
        /// answering yes, and a separate variant is exactly what those
        /// `matches!(…, WriteStmt::Set { .. })` arms would silently stop
        /// seeing. A binding names a statement's result; it does not
        /// change what the statement is.
        ///
        /// Only a CREATE may be named, which is a *derived* fact and so
        /// not the parser's to check — `unbindable_write` is raised at
        /// registration where the mode is derived (`ordo_plan::writes`).
        /// `del` cannot be named at all, and that one IS the parser's:
        /// there is no row to name, in any mode, ever.
        binding: Option<Ident>,
        entity: Ident,
        cond: Option<Condition>,
        fields: Vec<(Ident, Expr)>,
        span: Span,
    },
    Del {
        entity: Ident,
        /// The rows this delete claims. The addressed brace form
        /// (`del E { id: $id }`) is parsed into the `id == $id` condition
        /// it means: the spec calls both spellings "addressed" with
        /// identical semantics, and one representation is one behaviour —
        /// two could drift.
        ///
        /// There is no `Option` here on purpose. **Rule 5 [settled]:** bare
        /// `del entity` does not parse, so a delete without a stated target
        /// is not representable in this tree at all. Delete-everything is
        /// spelled `del entity(true)` — visible to a reader and to grep.
        cond: Condition,
        /// The explicit `subtree` word: this delete cascades to every
        /// descendant of the anchor (§tree, "Deleting tree parents"
        /// [settled]).
        ///
        /// A flag on `Del` rather than a variant of its own, deliberately.
        /// Everything that asks "is this statement a delete?" — the op
        /// kind on the wire, the mode label, the entity-touched analysis —
        /// must answer yes for a subtree delete too, and a separate variant
        /// is exactly what those `matches!(…, WriteStmt::Del { .. })` arms
        /// would silently stop seeing. The flag also fails in the safe
        /// direction: a consumer that ignores it plans an ordinary
        /// addressed delete, which is refused with the child count. A
        /// forgotten `subtree` under-deletes; it can never over-delete.
        subtree: bool,
        span: Span,
    },
}

/// An op body is a read pipeline or a write group — never both. A write
/// group's statements land in ONE command group: both or neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpBody {
    Read(Pipeline),
    Write(Vec<WriteStmt>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpDecl {
    pub name: Ident,
    pub params: Vec<ParamDecl>,
    pub flags: OpFlags,
    pub body: OpBody,
    pub span: Span,
}

impl OpDecl {
    /// The read pipeline, for the composition machinery. Write ops are not
    /// composable sources and answer None.
    pub fn read_pipeline(&self) -> Option<&Pipeline> {
        match &self.body {
            OpBody::Read(p) => Some(p),
            OpBody::Write(_) => None,
        }
    }
}

// ── pipelines ────────────────────────────────────────────────────────────

/// One argument inside a source's parenthesis list. The call form mixes named
/// parameter bindings with conditions in one comma list —
/// `cheapActive($cap: $cap, category under $root)` — so the two share a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceArg {
    /// `$cap: <expr>`
    Named {
        name: Ident,
        value: Expr,
        span: Span,
    },
    /// A bare condition: the entity-form filter, or a call-form refinement.
    Cond(Condition),
}

/// A projection item. `[*]`, `[*, -n]`, `[name]`, `[label: name]` and
/// `[sale: price - 500]` are all this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjItem {
    /// `*` — every output field of the source.
    Star { span: Span },
    /// `-n` — everything-minus.
    Exclude { name: Ident, span: Span },
    /// An output field addressed by path — `name`, or `customer.name`
    /// dotting through a ref. The output takes the last segment's name.
    Field { path: Vec<Ident>, span: Span },
    /// `out: <expr>` — rename when the expr is a bare path, computed
    /// otherwise. The binder decides which; the parser does not care.
    Named { out: Ident, value: Expr, span: Span },
}

/// One source in a source list: an entity or a registered op, optionally
/// called, optionally projected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: Ident,
    /// Present iff the source carried a parenthesis list.
    pub args: Vec<SourceArg>,
    pub projection: Option<Vec<ProjItem>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    pub expr: Expr,
    /// Leading `-`.
    pub desc: bool,
    pub span: Span,
}

/// `group customer { spent: sum total, n: count }` — one aggregate binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggDecl {
    pub out: Ident,
    pub func: Ident,
    /// `count` takes none; `sum total` takes one.
    pub arg: Option<Expr>,
    /// The second argument of the two-argument aggregates: `percentile`'s
    /// fraction (`percentile(total, 0.5)`) and `joined`'s separator
    /// (`joined(name, ", ")`). Both are literals, checked at parse time.
    pub arg2: Option<Expr>,
    /// `count(total > 5000)` — the conditional aggregate: count rows
    /// meeting the condition. Only `count` admits one.
    pub cond: Option<Condition>,
    pub span: Span,
}

/// The forms of a `match` step: duplicate-ref choice, let-key choice, and
/// the two existence operators. `any`/`none` are semi/anti-joins and are
/// NEVER preservation markers (ruling 3, amendment 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchKind {
    Any(Source),
    None(Source),
    Ref { path: Vec<Ident> },
    On { col: Ident },
}

/// `let name = <pipeline>` — a named intermediate, lowered as a CTE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetDecl {
    pub name: Ident,
    pub pipeline: Pipeline,
    pub span: Span,
}

/// How many rows a `take` keeps.
///
/// DL "Other steps" [settled]: *"`take $n` — legal: the count may be a
/// parameter, and the parameter must carry a bound (`$n: int <= 200`) so
/// the op's cost class is still statically provable. An unbounded
/// parameter here is `unbounded_take`."*
///
/// The bound is not decoration: it is the ONLY evidence the registration
/// gate has for the op's row bound, and it is only true because the door
/// enforces the same bound on the value before planning. Registration and
/// the door are two halves of one proof — either alone is a lie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeCount {
    Lit { n: i64, span: Span },
    Param { name: Ident },
}

impl TakeCount {
    pub fn span(&self) -> Span {
        match self {
            TakeCount::Lit { span, .. } => *span,
            TakeCount::Param { name } => name.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `| cond` — a post-source filter stage.
    Filter {
        cond: Condition,
        span: Span,
    },
    Sort {
        keys: Vec<SortKey>,
        span: Span,
    },
    /// `take 3`, `take $n`, and `take[plants.id] 1`.
    Take {
        keys: Vec<Expr>,
        n: TakeCount,
        span: Span,
    },
    Group {
        keys: Vec<Expr>,
        aggs: Vec<AggDecl>,
        span: Span,
    },
    /// `reduce { … }` — a group with no keys: exactly one row (ruling 1).
    Reduce {
        aggs: Vec<AggDecl>,
        span: Span,
    },
    Distinct {
        span: Span,
    },
    Match {
        kind: MatchKind,
        span: Span,
    },
    /// `append <source>` — one source form per arm (G8); a branch needing
    /// its own steps is a `let` first.
    Append {
        source: Source,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    /// `let` bindings preceding the source line.
    pub lets: Vec<LetDecl>,
    /// One or more sources on the opening line.
    pub sources: Vec<Source>,
    pub steps: Vec<Step>,
    pub span: Span,
}

// ── routines ─────────────────────────────────────────────────────────────

/// What wakes a routine (DL §7's two primitives, written inside the
/// routine as §8 spells them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutineTrigger {
    /// `trigger every 5m`, `trigger every day 09:00Z`. Nobody outside chose
    /// the moment, so guards pass (R-G20).
    Clock(Schedule),
    /// `trigger when notes.tasks.created debounce 60s`. The run may touch
    /// only rows reachable from the changed rows unless the `run` header
    /// says `unguarded` (R-G20).
    Change {
        pattern: ListenerPattern,
        modifiers: Modifiers,
    },
}

/// `routine name [in domain] { trigger … run[(params)] [flags]: <body> }`.
///
/// The body is a write group, the statements a write op takes, or a read
/// pipeline followed by `each ->` and the write statements to run for each
/// row it produces (DL §8, "`each ->` scope"). Either way a run lands as one
/// command group attributed `routine:<name>@<version>`. The parameters are
/// the runtime's to supply: `$now` and `$today`, the run's own instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineDecl {
    pub name: Ident,
    /// `in <domain>`, as written. When present it must name the program's
    /// domain.
    pub domain: Option<Ident>,
    pub trigger: RoutineTrigger,
    pub trigger_span: Span,
    pub params: Vec<ParamDecl>,
    /// The `run` header's flags (DL §8, "The `run` header takes flags"):
    /// `unguarded` lifts a change routine's row bound, `nopause` runs it in
    /// a paused tenant, and `unbounded` / `reproducible` mean what they
    /// mean on an op.
    pub flags: OpFlags,
    /// `<pipeline> each ->`: the rows the body runs once for. `None` is a
    /// plain write body, run once.
    ///
    /// With a pipeline, [`RoutineDecl::body`] is the emission. Inside its
    /// braces, and in the `{ id: … }` of an addressed `del`, the names in
    /// scope are the fields of the row the pipeline produced, and nothing
    /// else. A statement's condition keeps the scope of the entity it
    /// writes (DL §4).
    pub each: Option<Pipeline>,
    pub body: Vec<WriteStmt>,
    /// `notify <level> "<text>"`, between the trigger and `run`: after a run
    /// commits, the people who hold `<level>` on a row it wrote are told by
    /// email. See [`RoutineNotify`].
    pub notify: Option<RoutineNotify>,
    pub span: Span,
}

/// `notify admin "A reminder is due"`: a routine tells people by email.
///
/// After a run commits, every row it created or changed is followed to the
/// row its `acl` lives on, and each person who holds `level` there gets one
/// message whose subject and text are `text`. Nothing from a row is in the
/// message: the text is a literal, so the detail stays in the app (DL §8,
/// "payload minimization (detail revealed in-app)").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineNotify {
    pub level: Ident,
    pub text: String,
    pub span: Span,
}
