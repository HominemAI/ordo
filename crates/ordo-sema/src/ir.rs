//! The bound IR: what the binder produces and the planner consumes.
//!
//! Everything here is resolved. A `Col` names a real alias and a real column;
//! a path that did not resolve never becomes IR. The planner therefore does
//! no name lookup at all — if it can be built, it can be printed.

use ordo_syntax::decl::ScalarTy;
use ordo_syntax::{BinOp, CmpOp, Expr, TextOp};

/// A bound expression. Literals keep their AST node so the exact source
/// spelling of a decimal survives to SQL — the language's arithmetic is exact
/// decimal and re-rendering through a float would destroy that.
/// What a blind-index digest is computed FROM. Held unresolved in the IR
/// so no key-derived material is ever baked into a compiled artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlindSource {
    /// A literal written in the source.
    Literal(String),
    /// A parameter, resolved when the call supplies it.
    Param(String),
    /// A `null` literal. Digests the reserved null sentinel
    /// (`ordo_crypto::NULL_SENTINEL`) rather than emitting SQL `NULL`, so
    /// the language's one equality — `null == null` is true — reaches the
    /// digest column with no special case, exactly as it reaches every
    /// other column through `IS NOT DISTINCT FROM`.
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BExpr {
    Col {
        alias: String,
        column: String,
        ty: ScalarTy,
    },
    Lit {
        value: Expr,
        ty: ScalarTy,
    },
    /// A parameter that was never bound to a value. Reaches SQL as a
    /// placeholder; the corpus binds every parameter at the call site, so
    /// this is the shape prepared statements will use, not a corpus path.
    Param {
        name: String,
        ty: ScalarTy,
    },
    Neg {
        expr: Box<BExpr>,
        ty: ScalarTy,
    },
    Binary {
        op: BinOp,
        lhs: Box<BExpr>,
        rhs: Box<BExpr>,
        ty: ScalarTy,
    },
    Fn {
        name: String,
        args: Vec<BExpr>,
        ty: ScalarTy,
    },
    Cond {
        arms: Vec<(BCond, BExpr)>,
        else_: Box<BExpr>,
        ty: ScalarTy,
    },
    /// An aggregate. `mask` is ruling 3's null-extension guard: over an
    /// outer-preserved join every aggregate must ignore the synthesized null
    /// row, or an empty parent counts 1 and a filtered count conjures a
    /// phantom. Lowered as FILTER (WHERE mask), plus coalesce-to-0 for sum.
    Agg {
        func: String,
        arg: Option<Box<BExpr>>,
        /// The second argument of the two-argument aggregates: `percentile`'s
        /// fraction and `joined`'s separator. Always a literal.
        arg2: Option<Box<BExpr>>,
        /// `joined` alone is order-sensitive; this is the pipeline's order at
        /// the group step (declared sorts, then the tiebreak), captured there
        /// because grouping replaces both. Empty for every other aggregate.
        order: Vec<Sort>,
        mask: Option<Box<BCond>>,
        ty: ScalarTy,
    },
    /// ROW_NUMBER() OVER (PARTITION BY … ORDER BY …) — take[keys].
    RowNumber {
        partition: Vec<BExpr>,
        order: Vec<Sort>,
        ty: ScalarTy,
    },
    /// A bytea literal (hex) — blind-index digests. Never user-written.
    /// An unresolved blind-index digest: "HMAC this value under this
    /// entity and field's index key".
    ///
    /// The binder emits the SITE and never the digest. Computing it at
    /// bind time would put key-derived bytes inside the artifact, before
    /// the access check has run and in a file the deploy diff shows — the
    /// opposite of the pinned order (plan → access check → key release →
    /// host stage). The executor resolves these against its own key
    /// provider, after the door.
    BlindDigest {
        source: BlindSource,
        entity: String,
        field: String,
        normalize: Option<String>,
        ty: ScalarTy,
    },
    ByteaLit {
        hex: String,
        ty: ScalarTy,
    },
    /// A scalar subquery over a reduce-ended CTE: `(SELECT col FROM cte)`.
    ScalarSub {
        cte: String,
        column: String,
        ty: ScalarTy,
    },
    /// A value inside a `json` document: `prefs.theme`, `prefs["my key"]`,
    /// `tags[0]` (DL §2, JSON documents). `doc` is the json field; `keys`
    /// are the steps into it, in order. A step that is all digits is an
    /// array position when it meets an array and a key when it meets an
    /// object, which is how Postgres's `#>` reads a path.
    ///
    /// A document has no schema type, so `ty` is the type the condition
    /// gave it from its other side: `Text`, `Int`, `Decimal` or `Bool`
    /// reads the value under a type guard (a value of another JSON type
    /// reads as absent), and `Json` is the raw sub-document, which `is
    /// null` and `contains` ask about. Only a condition builds one.
    DocPath {
        doc: Box<BExpr>,
        keys: Vec<String>,
        ty: ScalarTy,
    },
}

impl BExpr {
    pub fn ty(&self) -> ScalarTy {
        match self {
            BExpr::Col { ty, .. }
            | BExpr::Lit { ty, .. }
            | BExpr::Param { ty, .. }
            | BExpr::Neg { ty, .. }
            | BExpr::Binary { ty, .. }
            | BExpr::Fn { ty, .. }
            | BExpr::Cond { ty, .. }
            | BExpr::Agg { ty, .. }
            | BExpr::RowNumber { ty, .. }
            | BExpr::ByteaLit { ty, .. }
            | BExpr::BlindDigest { ty, .. }
            | BExpr::ScalarSub { ty, .. }
            | BExpr::DocPath { ty, .. } => *ty,
        }
    }

    /// Does this expression contain an aggregate anywhere?
    pub fn has_agg(&self) -> bool {
        match self {
            BExpr::Agg { .. } => true,
            BExpr::Neg { expr, .. } => expr.has_agg(),
            BExpr::Binary { lhs, rhs, .. } => lhs.has_agg() || rhs.has_agg(),
            BExpr::Fn { args, .. } => args.iter().any(|a| a.has_agg()),
            BExpr::Cond { arms, else_, .. } => {
                arms.iter().any(|(_, v)| v.has_agg()) || else_.has_agg()
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BCond {
    Cmp {
        op: CmpOp,
        lhs: BExpr,
        rhs: BExpr,
    },
    In {
        expr: BExpr,
        list: Vec<BExpr>,
    },
    Text {
        op: TextOp,
        lhs: BExpr,
        rhs: BExpr,
    },
    IsNull {
        expr: BExpr,
        negated: bool,
    },
    And(Vec<BCond>),
    Or(Vec<BCond>),
    Not(Box<BCond>),
    /// A bare boolean expression used as a condition (G7).
    Bare(BExpr),
    /// Semi/anti-join: `match any` / `match none`. Correlation predicates
    /// live inside `query.filters`, referencing the outer alias.
    Exists {
        query: Box<Query>,
        negated: bool,
    },
    /// Tree containment over the stored `$path` column (ruling 5): `expr`
    /// is the node id, `anchor` the boundary id, `above` flips direction.
    TreeWalk {
        expr: BExpr,
        tree: String,
        anchor: BExpr,
        above: bool,
    },
}

/// One column of the query's result, in output order.
#[derive(Debug, Clone, PartialEq)]
pub struct OutField {
    pub name: String,
    pub expr: BExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sort {
    pub expr: BExpr,
    pub desc: bool,
}

/// A join produced by dotting through a ref. Always a many→one hop, so always
/// a LEFT JOIN: dotting through a nullable ref must not drop the row.
#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    pub table: String,
    pub alias: String,
    /// `<from_alias>.<from_column> = <alias>.id`
    pub from_alias: String,
    pub from_column: String,
    /// The TARGET's read guard, compiled against `alias`, rendered in the
    /// join's `ON`.
    ///
    /// **This field did not exist, and its absence was GC007 / S6-001.** A
    /// listed source gets its guard from `entity_source`; a row the compiler
    /// brought in by dotting through a ref got none, so `task.project.name`
    /// evaluated `tasks`' guard and never `projects`'. The guard of the row
    /// you start from says nothing about the row you arrive at.
    ///
    /// In the `ON` and not the `WHERE`, for the reason the space predicate
    /// is: the join is LEFT so that dotting through a nullable ref keeps the
    /// row, and a `WHERE` on the right-hand alias would discard exactly the
    /// rows the outer join preserved. With the guard in the `ON`, a target
    /// the caller may not read is indistinguishable from a ref that is null:
    /// the row stays, and every column reached through the hop is null
    /// (ruled 2026-09-26, `dotpath-guard-semantics`).
    ///
    /// `None` is one of three things, all decided where the join is built:
    /// the target has no guard, the body is `unguarded`, or the provenance
    /// is not a caller's.
    pub guard: Option<BCond>,
}

/// A multi-source join edge — always LEFT (the multi-source form is
/// outer-preserving with no keyword). `many_side` marks the fanning end,
/// which is what bare `count` counts and what the mask tests.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceJoin {
    pub from: From,
    pub on: Vec<(BExpr, BExpr)>,
    pub many_side: bool,
    /// The read guard of a **bridge** entity, rendered in the join's `ON`.
    ///
    /// Set only when `from` is a table the compiler inserted to connect two
    /// listed sources. A listed source needs nothing here: it was bound by
    /// `entity_source`, which put its guard in its own filters, and a source
    /// with filters is lowered as a sub-select that carries them.
    ///
    /// **A bridge is the one row in a multi-source query that nobody
    /// listed**, so nothing bound it and nothing guarded it (S6-001, found
    /// by the 2026-09-28 security review and verified against the running
    /// binary: a member read which person was assigned to which place
    /// through an `assignments` row their own guard forbids). It is
    /// [`Join::guard`]'s defect on the other compiler-added node, and it
    /// was worse in one respect: the op manifest reported the bridge as
    /// guarded, because reach is computed from the schema and the guard was
    /// dropped in the binder.
    pub guard: Option<BCond>,
}

/// A named sub-pipeline lowered as a WITH binding; referenced, not copied,
/// so a let used twice emits once.
#[derive(Debug, Clone, PartialEq)]
pub struct Cte {
    pub name: String,
    pub query: Query,
}

#[derive(Debug, Clone, PartialEq)]
pub enum From {
    Table {
        entity: String,
        alias: String,
    },
    /// A reference to a WITH binding.
    CteRef {
        name: String,
        alias: String,
    },
    /// UNION ALL arms (`append`), wrapped so the sort/take/distinct
    /// machinery sees one query shape.
    Union {
        arms: Vec<Query>,
        alias: String,
    },
    /// A callee that could not be merged — it had a `take`, a `group`, or a
    /// `distinct`, so its row set must be settled before the caller filters.
    /// This is post-take semantics made structural rather than hoped for.
    Sub {
        query: Box<Query>,
        alias: String,
    },
}

impl From {
    pub fn alias(&self) -> &str {
        match self {
            From::Table { alias, .. }
            | From::Sub { alias, .. }
            | From::CteRef { alias, .. }
            | From::Union { alias, .. } => alias,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub keys: Vec<BExpr>,
}

/// A `take`'s row count as the plan carries it.
///
/// `take $n` (DL "Other steps" [settled]) keeps the count in a parameter,
/// so the number is not in the plan — but the op's row bound still has to
/// be provable at registration, which is what the parameter's declared
/// bound is for. Both facts ride here: the slot the statement binds, and
/// the ceiling the gate proved against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Limit {
    Lit(i64),
    /// `name` is a parameter of the op being bound (or, under composition,
    /// of the outermost op whose argument reached this slot). `max` is its
    /// declared `<=` bound — `Some` wherever the op is known, which is
    /// every path registration and the artifact take. `None` means the
    /// binder had no op to ask (a bare pipeline in the harness), and the
    /// row bound is then Unbounded, which is the honest answer.
    Param {
        name: String,
        max: Option<i64>,
    },
}

impl Limit {
    /// The provable ceiling on returned rows: the literal, or the
    /// parameter's declared bound.
    pub fn at_most(&self) -> Option<i64> {
        match self {
            Limit::Lit(n) => Some(*n),
            Limit::Param { max, .. } => *max,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub from: From,
    pub joins: Vec<Join>,
    /// Conjunction. An empty list means no WHERE clause.
    pub filters: Vec<BCond>,
    /// Post-group filters — a `| cond` after `group` filters buckets, and
    /// bucket conditions reference aggregates, which SQL forbids in WHERE.
    /// Found by corpus C056: the naive lowering emitted `WHERE sum(...)`.
    pub having: Vec<BCond>,
    pub group: Option<Group>,
    pub outputs: Vec<OutField>,
    /// Outputs whose values arrive as sealed envelopes and must be opened
    /// **host-side**: `(output name, entity, field)`. The entity and field
    /// are the associated data the envelope is bound to, so a payload
    /// moved to another column refuses to open rather than decrypting into
    /// the wrong place.
    ///
    /// A non-empty list is what makes a plan a HOST-class plan: rows leave
    /// the database as ciphertext and become plaintext in this process,
    /// which is a different cost and a different disclosure from a plain
    /// SQL read, and the cost gate says so.
    pub host_open: Vec<(String, String, String)>,
    /// WITH bindings — outermost query only.
    pub ctes: Vec<Cte>,
    /// Multi-source join edges (distinct from dot-hop `joins`).
    pub source_joins: Vec<SourceJoin>,
    pub sort: Vec<Sort>,
    /// True when `sort` was inherited from a wrapped callee rather than
    /// declared by this pipeline. A declared sort REPLACES an inherited one
    /// (R-C4: the callee's order stands only when the caller declares none);
    /// appending after it would order by the callee first and make the
    /// caller's sort a mere tiebreak.
    pub sort_inherited: bool,
    pub limit: Option<Limit>,
    pub distinct: bool,
    /// The implicit ordering tiebreak (R-C4/R-C7), appended after every
    /// declared sort key so the order is total.
    ///
    /// A row pipeline tiebreaks on the anchor row id. A grouped pipeline has
    /// no row identity, so it tiebreaks on its group keys instead — and a
    /// grouped callee read through a subquery must carry those keys outward,
    /// or the outer query silently loses the guarantee.
    pub tiebreak: Vec<BExpr>,
    /// The columns the database declares `NOT NULL`, by table. Set by the
    /// binder on the query it hands out, from the same column list the DDL
    /// is printed from; nested queries leave it `None` and the planner reads
    /// the outermost one.
    ///
    /// This is evidence for the planner's never-null rule
    /// (`ordo_plan::never_null`), which decides where `==` may compile to a
    /// plain `=` that an index can serve. `None` proves nothing, so every
    /// comparison keeps the null-safe form: a query built by hand, or by a
    /// path that forgot to fill this, is slower and never wrong.
    pub not_null: Option<NotNullColumns>,
}

/// Table name to the set of its columns declared `NOT NULL`. Shared, because
/// the planner clones query nodes freely.
pub type NotNullColumns =
    std::sync::Arc<std::collections::BTreeMap<String, std::collections::BTreeSet<String>>>;
