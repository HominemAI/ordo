//! **The ratchet.** Every closed vocabulary the language has, and what it is
//! called in TypeScript.
//!
//! # Why this file cannot contain a `_ =>` arm
//!
//! `ordo-exec/src/export.rs` set the precedent and states the reason: *"Value
//! encoding is an exhaustive match on `ScalarTy`. A new scalar type does not
//! silently fall through to a string: it **fails to compile** here until
//! somebody decides what its JSON looks like. New *fields* need no decision;
//! a new *type in the language* is a decision, and this is where it gets
//! made rather than defaulted."*
//!
//! The same argument, one layer out. A generated SDK and a generated builder
//! are the language's surface in another language, and a wildcard arm here
//! would mean a new keyword, comparison operator or scalar type arriving in
//! `token.rs` and reaching TypeScript as *nothing at all* — silently, with
//! both packages still compiling and both test suites still green. The author
//! of the new word would never learn that two client packages exist.
//!
//! So every function below matches a closed enum with one arm per variant and
//! no catch-all, and `tests/ratchet.rs` scans this file's source for the
//! wildcard forms so the rule cannot be relaxed by someone in a hurry. Adding
//! a variant to `Keyword`, `ScalarTy`, `CmpOp`, `TextOp`, `BinOp`,
//! `DurationUnit` or `EventKind` therefore **fails `cargo build`** until
//! someone decides its TypeScript spelling — including the decision "it is
//! not offered, and here is why", which is a legal answer and the one most of
//! the declaration keywords get.
//!
//! # Absence is a decision, and it is published
//!
//! [`Spelling::Absent`] is not a hole; it is the CDK's refusal, and the
//! generator writes every one of them into `vocabulary.ts` with its reason.
//! Words such as `app` and `page` are absent because **the language
//! does not parse them**, so an absent method is the language's own refusal
//! reaching the builder, not a gap in it. That distinction is the whole of
//! Track C's "a frontend, never a second semantics": a builder that offered
//! a word no `.ordo` file can say would express something the parser refuses,
//! which is exactly the property the plan says isomorphism is tested for.

use ordo_syntax::decl::ScalarTy;
use ordo_syntax::token::{DurationUnit, Keyword};
use ordo_syntax::trigger::EventKind;
use ordo_syntax::{BinOp, CmpOp, TextOp};

/// What a word from a closed language vocabulary is called in TypeScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spelling {
    /// The CDK offers it under this name — a method, a free function, or a
    /// modifier. The string is what an author types.
    Offered(&'static str),
    /// The word is reachable, but only inside another spelling: `is` and
    /// `null` are reached through `.isNull()`, `match` through the four
    /// `.match*` steps. The string names the spelling that carries it.
    Fused(&'static str),
    /// The CDK does not offer it, and this is why.
    Absent(Absent),
}

/// Why a word of the language has no TypeScript spelling.
///
/// The two reasons are owed differently, which is why they are two variants
/// rather than one string — the same split `docs_parse.rs` draws between
/// "settled and unbuilt" and "[lean], not settled", and for the same reason:
/// conflating them turns a deliberate exclusion into a work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Absent {
    /// **The language does not parse this**, so the builder must not offer
    /// it. An absent method is the language's refusal, arriving one layer
    /// earlier. Nothing is owed until the grammar moves.
    DoesNotParse(&'static str),
    /// The language parses it and this increment does not cover it. A real
    /// debt, and the crate README and the checklist both carry it.
    OutOfScope(&'static str),
}

impl Spelling {
    /// The name an author types, or `None` when the word is not offered.
    pub fn offered(self) -> Option<&'static str> {
        match self {
            Spelling::Offered(name) => Some(name),
            Spelling::Fused(_) | Spelling::Absent(_) => None,
        }
    }

    /// A one-word tag for the generated manifest.
    pub fn tag(self) -> &'static str {
        match self {
            Spelling::Offered(_) => "offered",
            Spelling::Fused(_) => "fused",
            Spelling::Absent(Absent::DoesNotParse(_)) => "does-not-parse",
            Spelling::Absent(Absent::OutOfScope(_)) => "out-of-scope",
        }
    }

    /// The prose that rides with it into `vocabulary.ts`.
    pub fn note(self) -> &'static str {
        match self {
            Spelling::Offered(name) | Spelling::Fused(name) => name,
            Spelling::Absent(Absent::DoesNotParse(why))
            | Spelling::Absent(Absent::OutOfScope(why)) => why,
        }
    }
}

// ── Keyword ──────────────────────────────────────────────────────────────

/// Every reserved word, and what the CDK calls it.
///
/// One arm per variant, no catch-all. A word added to `token.rs` stops this
/// crate compiling until somebody answers "and what is it called in
/// TypeScript?" — which is the only question that matters here, and the one
/// nobody would otherwise be asked.
pub fn keyword(k: Keyword) -> Spelling {
    use Absent::{DoesNotParse, OutOfScope};
    use Spelling::{Absent as A, Fused, Offered};
    match k {
        // ── logic and pipeline ──
        Keyword::And => Offered("and"),
        Keyword::Or => Offered("or"),
        Keyword::Not => Offered("not"),
        Keyword::In => Offered(".in"),
        // `is` never stands alone: `expr is null` and `expr is not null` are
        // the only null tests the grammar has, so the builder has the only
        // two spellings there are.
        Keyword::Is => Fused(".isNull() / .isNotNull()"),
        Keyword::Null => Fused("nul() — the literal; .isNull() — the test"),
        Keyword::True => Fused("lit(true)"),
        Keyword::False => Fused("lit(false)"),
        // The four `match` forms are four methods rather than one method with
        // a mode argument: they take different operands, and a mode argument
        // would be a place for an unknown mode to arrive at run time.
        Keyword::Match => Fused(".matchAny/.matchNone/.matchRef/.matchOn"),
        Keyword::Any => Offered(".matchAny"),
        Keyword::None => Offered(".matchNone"),
        Keyword::On => Offered(".matchOn"),
        Keyword::Group => Offered(".group"),
        Keyword::Reduce => Offered(".reduce"),
        Keyword::Sort => Offered(".sort"),
        Keyword::Take => Offered(".take"),
        Keyword::Append => Offered(".append"),
        Keyword::Let => Offered("let_"),
        // `distinct` is the one word in two inventories (a step and an
        // aggregate), so it gets the two spellings the language gives it.
        Keyword::Distinct => Offered(".distinct / agg.distinct"),
        Keyword::Under => Offered(".under"),
        Keyword::Above => Offered(".above"),
        Keyword::Since => Offered(".since"),
        Keyword::Contains => Offered(".contains"),
        Keyword::Starts => Offered(".starts"),
        Keyword::Matches => Offered(".matches"),
        Keyword::Set => Offered("w.set"),
        Keyword::Del => Offered("w.del"),
        Keyword::Subtree => Offered(".subtree"),
        // A routine's `<pipeline> each -> <writes>` body. `each` over a
        // `list of { … }` parameter is [lean] (definition-language.md §1)
        // and does not parse; the word is offered for the form that does.
        Keyword::Each => Offered("each"),

        // ── declarations, clauses and field modifiers ──
        Keyword::Op => Offered("op"),
        Keyword::Entity => Offered("entity"),
        Keyword::Routine => Offered("routine"),
        Keyword::Domain => Offered("domain"),
        Keyword::When => Offered("when"),
        Keyword::Every => Offered("every / daily"),
        Keyword::Trigger => Fused("the `trigger` option of routine()"),
        Keyword::Check => Offered(".check"),
        Keyword::Index => Offered(".index"),
        Keyword::Unique => Offered(".unique"),
        Keyword::Sealed => Offered(".sealed"),
        Keyword::Indexed => Offered(".indexed"),
        Keyword::Owned => Offered(".owned"),
        Keyword::Tree => Offered(".tree"),
        Keyword::Normalize => Offered(".normalize"),
        Keyword::Whole => A(OutOfScope(
            "`whole` is a special form, not a pipeline step, and it does not \
             compose — a builder for it is a second skeleton, not vocabulary \
             poured into this one. See the crate README",
        )),

        // ── authorization and platform blocks ──
        Keyword::Acl => Offered(".acl"),
        Keyword::Access => Offered(".accessRead / .accessWrite"),
        Keyword::App => A(DoesNotParse(
            "`app` blocks are `ui-gen.md` §5, whose binding grammar is normative \
             and whose block set is explicitly still open",
        )),
        Keyword::Page => A(DoesNotParse(
            "`page` blocks are `ui-gen.md` §5 with `app`: the binding grammar is \
             normative and the block set is explicitly still open",
        )),
        Keyword::Principal => A(DoesNotParse(
            "`principal` is under §8's [lean] spelling pin; the mechanism is \
             built, the declaration is not fixed",
        )),
        Keyword::Client => A(DoesNotParse(
            "`client` is under §8's [lean] spelling pin, like `principal`: the \
             mechanism is built and the declaration form is not fixed",
        )),
        Keyword::Auth => A(DoesNotParse(
            "`auth` is under §8's [lean] spelling pin; no declaration form for it \
             parses today",
        )),
        Keyword::Policy => A(DoesNotParse(
            "`policy` is under §8's [lean] spelling pin; no declaration form for \
             it parses today",
        )),
        Keyword::Compliance => A(DoesNotParse(
            "`compliance` is under §8's [lean] spelling pin; no declaration form \
             for it parses today",
        )),
        Keyword::Channel => A(DoesNotParse(
            "`channel` is under §8's [lean] spelling pin. Its semantics are \
             settled and there is no engine mechanism behind it either: the only \
             `channel` in this workspace is the invalidation bus, a different \
             sense of the word",
        )),
        Keyword::Space => A(DoesNotParse(
            "`space` is reserved so it cannot be a clause head by accident; no \
             declaration form for it parses",
        )),

        // ── registration flags ──
        Keyword::Unguarded => Offered(".unguarded"),
        Keyword::Unbounded => Offered(".unbounded"),
        Keyword::Reproducible => Offered(".reproducible"),
        Keyword::Nopause => Offered(".nopause"),
        // One word, three positions: `t.text().public()` on a field,
        // `.public()` / `.publicAll()` (`public*`) on an entity, `.public()`
        // on an op (`plan-syntax-additions.md` §1).
        Keyword::Public => Offered(".public"),
        Keyword::Filled => A(OutOfScope(
            "`filled` parses on a parameter (`$today: date filled`) and the builder \
             does not offer it yet",
        )),
        Keyword::Cond => Offered("cond"),
    }
}

// ── ScalarTy ─────────────────────────────────────────────────────────────

/// How one scalar type reaches TypeScript: the CDK's declaration
/// constructor, the SDK's value type, and the wire shape §5 pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scalar {
    /// `t.<ctor>()` in the CDK. `None` means the type has **no declaration
    /// form in the language**, so the builder must not offer one.
    pub ctor: Option<&'static str>,
    /// The TypeScript type a value of it takes in the SDK.
    pub ts: &'static str,
    /// What `api-engine-design.md` §5 says the wire shape is — the sentence
    /// that ends up in the generated doc comment.
    pub wire: &'static str,
}

/// The scalar table. Exhaustive on `ScalarTy`, for `export.rs`'s reason.
///
/// `Decimal` is the interesting arm and the one that proves the table is a
/// projection rather than a convenience: DL §2 marks it *expression only*, so
/// it has no declaration form, and `ctor: None` is how the builder inherits
/// that refusal. A wildcard arm would have given it `t.decimal()` and let a
/// CDK author write a column the compiler rejects.
pub fn scalar(t: ScalarTy) -> Scalar {
    match t {
        ScalarTy::Text => Scalar {
            ctor: Some("text"),
            ts: "string",
            wire: "JSON string; longer than a declared `text(n)` is `bound_violation`",
        },
        ScalarTy::Int => Scalar {
            ctor: Some("int"),
            ts: "number",
            wire: "JSON integer",
        },
        ScalarTy::Money(_) => Scalar {
            ctor: Some("money"),
            // Not `bigint`. `tooling-and-embedding-plan.md` §B3 proposed
            // "bigint-minor-units in TS" before the wire was settled;
            // `api-engine-design.md` §5 settles it as an exact decimal
            // string, and the descriptor's `max` is a decimal digit string
            // too. A generated client that parsed it into a bigint would be
            // choosing a second representation for the value it is supposed
            // to carry unchanged.
            ts: "string",
            wire: "exact decimal string (`\"-12.50\"`) — never a float, never a number",
        },
        ScalarTy::Decimal(_) => Scalar {
            // Storable since 27ak as `decimal(p)`, for rates and unit
            // prices. Exact, so a string for the reason money is one.
            ctor: Some("decimal"),
            ts: "string",
            wire: "exact decimal string with at most the declared places (`\"0.0825\"`) — never a float",
        },
        ScalarTy::Bool => Scalar {
            ctor: Some("bool"),
            ts: "boolean",
            wire: "JSON boolean",
        },
        ScalarTy::Date => Scalar {
            ctor: Some("date"),
            ts: "string",
            wire: "`\"YYYY-MM-DD\"`, validated",
        },
        ScalarTy::Datetime => Scalar {
            ctor: Some("datetime"),
            ts: "string",
            wire: "`\"YYYY-MM-DDTHH:MM:SSZ\"` — UTC, the `Z` mandatory",
        },
        // The unit is part of the type, so the constructor takes it:
        // `t.duration('ms')`. It is not a default, because two durations of
        // different units are different types and the engine converts
        // between them nowhere.
        ScalarTy::Duration(_) => Scalar {
            ctor: Some("duration"),
            ts: "string",
            wire: "`\"<int><unit>\"`, units exactly `ms s m h d w mo y`",
        },
        ScalarTy::Id => Scalar {
            ctor: Some("id"),
            // Branded per entity where the descriptor says which entity
            // (`id_entity`); a bare `id` with no entity stays `string`.
            ts: "string",
            wire: "typed id string (`prefix_body`)",
        },
        // A blob field is declarable, so it has a constructor. Its **value**
        // on the wire is the hex handle and nothing else: the bytes never
        // ride a JSON envelope, in either direction. Uploading goes through
        // the blob door, and downloading is `GET /blob/{ref}` with the
        // handle a read projected — which is why `ts` is `string` rather
        // than some `Blob`-shaped type the SDK would have to fabricate.
        ScalarTy::Blob => Scalar {
            ctor: Some("blob"),
            ts: "string",
            wire: "opaque hex handle; the bytes move through the blob door, never in JSON",
        },
        // An address, and a plain one. `uri` is text-shaped everywhere
        // precisely because it has no custody: nothing is held, so nothing
        // is opaque.
        ScalarTy::Uri => Scalar {
            ctor: Some("uri"),
            ts: "string",
            wire: "JSON string — an address the platform references and does not hold",
        },
        // A JSON document travels as JSON itself, never as a string holding
        // JSON (28b). `JsonValue` is the SDK's own type for any JSON value,
        // so a caller cannot hand the wire a `Date`, a function or
        // `undefined` that `JSON.stringify` would quietly change.
        ScalarTy::Json => Scalar {
            ctor: Some("json"),
            ts: "JsonValue",
            wire: "any JSON value, carried as JSON; larger or deeper than the deployment's limit is `bound_violation`",
        },
    }
}

/// Every scalar type that has a declaration form, in declaration order.
///
/// Built by walking a written-down list of every variant — which the
/// exhaustive match in [`scalar`] is what keeps honest, because a new variant
/// missing from this list would still have to be given a `Scalar` above.
pub fn declarable_scalars() -> Vec<(ScalarTy, Scalar)> {
    all_scalars()
        .into_iter()
        .filter(|(_, s)| s.ctor.is_some())
        .collect()
}

/// Every `ScalarTy`, with the duration unit standing for the family.
///
/// It **is** [`ScalarTy::ALL`] now, and that is a repair rather than a
/// tidy-up. This function used to carry its own written-out variant list,
/// with a comment explaining that `ScalarTy` could not have a `const ALL`
/// because `Duration(DurationUnit)` is not a unit variant. That was true of
/// `word_enum!` and not of the enum: `Duration(DurationUnit::Ms)` is a
/// perfectly good const expression, and the missing `ALL` was the reason
/// `editors/ordo.tmLanguage.json`'s type list had no parity test while five
/// other inventories did. The list now lives on the producer, one copy,
/// checked against the enum by an exhaustive match in `ordo-syntax`.
pub fn all_scalars() -> Vec<(ScalarTy, Scalar)> {
    ScalarTy::ALL.iter().map(|t| (*t, scalar(*t))).collect()
}

// ── operators ────────────────────────────────────────────────────────────

/// The comparison operators, as builder methods on a value.
///
/// The names are the sketch's (`f.x.eq(…)`, `f.billed.gt(…)`) and the
/// spelling each produces is `CmpOp::as_str()`, never retyped here — the
/// operator table stays the parser's.
pub fn cmp(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "eq",
        CmpOp::Ne => "ne",
        CmpOp::Lt => "lt",
        CmpOp::Le => "le",
        CmpOp::Gt => "gt",
        CmpOp::Ge => "ge",
    }
}

pub const ALL_CMP: &[CmpOp] = &[
    CmpOp::Eq,
    CmpOp::Ne,
    CmpOp::Lt,
    CmpOp::Le,
    CmpOp::Gt,
    CmpOp::Ge,
];

/// Text matching stays in operators, never functions (spec §4) — so these
/// are methods on a value beside `eq`, not entries in `fn.`.
pub fn text_op(op: TextOp) -> &'static str {
    match op {
        TextOp::Contains => "contains",
        TextOp::Starts => "starts",
        TextOp::Matches => "matches",
    }
}

pub const ALL_TEXT_OPS: &[TextOp] = &[TextOp::Contains, TextOp::Starts, TextOp::Matches];

/// Arithmetic. `+` doubles as text concatenation and the builder does not
/// split it either: typing is sema's job, one spelling per concept.
pub fn bin_op(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
    }
}

pub const ALL_BIN_OPS: &[BinOp] = &[BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div];

// ── duration units ───────────────────────────────────────────────────────

/// A duration unit's TypeScript spelling is its own word: the units are a
/// closed string union in the generated types, so `t.duration('mo')` is a
/// compile error away from `t.duration('mos')`.
///
/// This match exists rather than a bare `as_str()` call so that a unit added
/// to `token.rs` lands here as a compile error too. The value it returns is
/// still the producer's — nothing is retyped, only acknowledged.
pub fn duration_unit(u: DurationUnit) -> &'static str {
    match u {
        DurationUnit::Ms
        | DurationUnit::S
        | DurationUnit::M
        | DurationUnit::H
        | DurationUnit::D
        | DurationUnit::W
        | DurationUnit::Mo
        | DurationUnit::Y => u.as_str(),
    }
}

// ── event kinds ──────────────────────────────────────────────────────────

/// The trigger vocabulary: seven words, written inside a `when('…')`
/// pattern string and not as words of their own.
///
/// The parser checks the kind against this closed set when it reads the
/// pattern (`unknown_event_kind`), so the builder carries the string and
/// offers no second list that could drift. This match stays so that an
/// eighth kind makes its author decide whether the builder should name it.
pub fn event_kind(k: EventKind) -> Spelling {
    let why = "event kinds are written inside the `when('<domain>.<entity>.<kind>')` \
               pattern string, and the parser checks them against its closed set; the \
               builder offers no second list of them";
    match k {
        EventKind::Created
        | EventKind::Converged
        | EventKind::Deleted
        | EventKind::Written
        | EventKind::Ingested
        | EventKind::Fed
        | EventKind::Reverted => Spelling::Absent(Absent::DoesNotParse(why)),
    }
}
