//! The cost gate's analysis half: venue classification and row bounds.
//!
//! Classification walks the bound IR — never SQL text — and assigns the
//! deploy-time class the catalog shows and the gate enforces. A read that
//! returns a sealed field is a host class. The executor serves
//! `bounded-host` one page at a time, one key release per page, and refuses
//! `unbounded-host` naming the field (27ao). Host work is projection-only:
//! a sealed field in a filter or a sort key is refused at bind unless a
//! blind index answers it in the database.
//!
//! Row bounds are evidence-based (DL ~1840): a `take n` bounds at n, a
//! keyless `reduce` provably yields one row, an equality on the anchor id
//! yields at most one. Everything else is Unbounded — which for sql-class
//! ops means "pageable, each page bounded by the runtime limit", and for
//! host classes would mean the `unbounded` acknowledgment. take[keys] is
//! deliberately NOT evidence (the ledger's undecided item: bounded only if
//! the key space is).
//!
//! **Keyset paging is separate evidence, and it is deliberately not part of
//! [`row_bound`]** — see [`keyset_paged`] and the ruling of 2026-09-05 in
//! `docs/corpus-rulings.md`. A paged plan bounds the rows one REQUEST
//! returns; `row_bound` answers about the rows the plan itself yields, and
//! the two are different quantities in exactly the places that matter
//! ([`refine_candidate_bound`] asks the bound question at a reduce's INPUT,
//! where no page window exists). So the registration gate accepts paging as
//! evidence and the prover does not.

use ordo_schema::Schema;
use ordo_sema::ir::{BCond, BExpr, From, Query};
use ordo_sema::{bind_op_generic, Registry, SemaError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostClass {
    Sql,
    BlindIndex,
    BoundedHost,
    UnboundedHost,
}

impl CostClass {
    /// The whole inventory, cheapest first. Declaration order **is** the
    /// order [`CostClass::join`] ranks by, so a new class lands in its
    /// place in the lattice by being declared in it.
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

    /// The inventory read backwards, so a class name has one way home.
    pub fn from_class(class: &str) -> Option<CostClass> {
        CostClass::ALL.iter().copied().find(|c| c.as_str() == class)
    }

    /// Position in the cost lattice. Written once, here, because
    /// [`classify_with`]'s "host dominates" rule and §7.1's `⊔` are the same
    /// ordering, and two spellings of one order is how they come to disagree.
    pub fn rank(self) -> u8 {
        CostClass::ALL
            .iter()
            .position(|c| *c == self)
            .expect("every class is in ALL") as u8
    }

    /// `⊔` — the join of two classes: composition costs what its most
    /// expensive part costs. This is the operator `api-engine-design.md`
    /// §7.1 writes as `effective = registered ⊔ class_of(refinement)`.
    pub fn join(self, other: CostClass) -> CostClass {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowBound {
    /// Provably at most this many rows.
    AtMost(i64),
    /// Grows with the space; pageable at runtime for sql-class plans.
    Unbounded,
}

impl RowBound {
    pub fn as_string(self) -> String {
        match self {
            RowBound::AtMost(n) => format!("<= {n}"),
            RowBound::Unbounded => "unbounded (pageable)".into(),
        }
    }
}

fn expr_has_blind(e: &BExpr) -> bool {
    match e {
        // An unresolved digest site is as much a blind-index plan as a
        // resolved one — classification runs on the artifact, which is
        // exactly where the digest is deliberately absent.
        BExpr::BlindDigest { .. } => true,
        BExpr::ByteaLit { .. } => true,
        BExpr::Col { column, .. } => column.ends_with("$bidx"),
        BExpr::Neg { expr, .. } => expr_has_blind(expr),
        BExpr::Binary { lhs, rhs, .. } => expr_has_blind(lhs) || expr_has_blind(rhs),
        BExpr::Fn { args, .. } => args.iter().any(expr_has_blind),
        // The audit's finding 3: a digest compare hides inside cond() arms,
        // which live in projections, filters, and sort keys alike.
        BExpr::Cond { arms, else_, .. } => {
            arms.iter()
                .any(|(c, v)| cond_has_blind(c) || expr_has_blind(v))
                || expr_has_blind(else_)
        }
        BExpr::Agg { arg, mask, .. } => {
            arg.as_deref().map(expr_has_blind).unwrap_or(false)
                || mask.as_deref().map(cond_has_blind).unwrap_or(false)
        }
        BExpr::RowNumber {
            partition, order, ..
        } => partition.iter().any(expr_has_blind) || order.iter().any(|s| expr_has_blind(&s.expr)),
        _ => false,
    }
}

fn cond_has_blind(c: &BCond) -> bool {
    match c {
        BCond::Cmp { lhs, rhs, .. } => expr_has_blind(lhs) || expr_has_blind(rhs),
        BCond::In { expr, list } => expr_has_blind(expr) || list.iter().any(expr_has_blind),
        BCond::Text { lhs, rhs, .. } => expr_has_blind(lhs) || expr_has_blind(rhs),
        BCond::IsNull { expr, .. } => expr_has_blind(expr),
        BCond::Bare(e) => expr_has_blind(e),
        BCond::And(v) | BCond::Or(v) => v.iter().any(cond_has_blind),
        BCond::Not(i) => cond_has_blind(i),
        BCond::Exists { query, .. } => query_has_blind(query),
        BCond::TreeWalk { expr, anchor, .. } => expr_has_blind(expr) || expr_has_blind(anchor),
    }
}

/// Whether any part of `q` compares through a blind index: a digest the
/// planner cannot render without the space's key. Asked apart from the cost
/// class, because a sealed field in the projection raises the class to a
/// host class while the lookup still rides the digest (reported fix 25ap).
pub fn query_has_blind(q: &Query) -> bool {
    q.filters.iter().any(cond_has_blind)
        || q.having.iter().any(cond_has_blind)
        || q.outputs.iter().any(|o| expr_has_blind(&o.expr))
        || q.sort.iter().any(|s| expr_has_blind(&s.expr))
        || q.group
            .as_ref()
            .map(|g| g.keys.iter().any(expr_has_blind))
            .unwrap_or(false)
        || q.ctes.iter().any(|c| query_has_blind(&c.query))
        || match &q.from {
            From::Sub { query, .. } => query_has_blind(query),
            From::Union { arms, .. } => arms.iter().any(query_has_blind),
            _ => false,
        }
        || q.source_joins.iter().any(|j| match &j.from {
            From::Sub { query, .. } => query_has_blind(query),
            _ => false,
        })
}

/// Is every row the host opens a row of the result, one for one?
///
/// True when nothing between the opened rows and the output folds or
/// multiplies them: no `group` or `reduce`, no `distinct`, no many-side
/// join, no host work inside a `let`, and a callee or `append` arm that
/// meets the same test. Then a page of output is a page of opened rows,
/// and the page bounds the host work. Anything else is refused the credit,
/// which is the conservative answer: the op needs `unbounded` or a `take`.
fn host_rows_are_page_rows(q: &Query) -> bool {
    if q.group.is_some() || q.distinct {
        return false;
    }
    if q.ctes.iter().any(|c| query_has_host(&c.query)) {
        return false;
    }
    if q.source_joins.iter().any(|j| {
        j.many_side
            || match &j.from {
                From::Sub { query, .. } => query_has_host(query),
                From::Union { arms, .. } => arms.iter().any(query_has_host),
                _ => false,
            }
    }) {
        return false;
    }
    match &q.from {
        From::Table { .. } | From::CteRef { .. } => true,
        From::Sub { query, .. } => !query_has_host(query) || host_rows_are_page_rows(query),
        From::Union { arms, .. } => arms
            .iter()
            .all(|a| !query_has_host(a) || host_rows_are_page_rows(a)),
    }
}

/// Does any part of this plan hand rows to the host to open?
fn query_has_host(q: &Query) -> bool {
    !q.host_open.is_empty()
        || q.ctes.iter().any(|c| query_has_host(&c.query))
        || match &q.from {
            From::Sub { query, .. } => query_has_host(query),
            From::Union { arms, .. } => arms.iter().any(query_has_host),
            _ => false,
        }
        || q.source_joins.iter().any(|j| match &j.from {
            From::Sub { query, .. } => query_has_host(query),
            _ => false,
        })
}

/// Venue classification. Composition takes the max, and the host classes
/// dominate: a plan that opens a sealed payload does work outside the
/// database no matter what else it does, and the catalog says so.
///
/// Bounded vs unbounded host is the row-bound question asked at the venue:
/// `bounded-host` means the number of payloads this plan will open is
/// provable from the plan itself. Unbounded-host is a plan whose host work
/// grows with the data, which is exactly what the `unbounded`
/// acknowledgment exists to make visible.
///
/// A fold never sits over host work. The binder clears a `group`'s or a
/// `reduce`'s obligations, because nothing sealed survives a fold, and
/// `distinct` over a sealed output is refused (ruled 2026-10-05, audit ask
/// 1). A sub-select keeps only the columns its outer query reads (reported
/// fix 25v), so an envelope a fold dropped is not selected below it
/// either. So [`row_bound`] answers for the rows the host opens.
///
/// # A page bounds the payloads a projection opens
///
/// Two settled facts meet here. DL §5 (Cost classes): *"`take` bounds the
/// host-side work in a **projection** — decrypting the sealed fields of
/// the rows that survive — and bounds nothing when the sealed work is in a
/// filter or a sort key."* And the 2026-09-05 ruling (`corpus-rulings.md`,
/// "Keyset paging counts as a row bound"): *"a plan is row-bounded if it
/// is served as a keyset page over a total order."* The door serves every
/// read one page at a time, so the rows whose payloads the host opens for
/// a request are the page's rows: a page bounds projection work exactly as
/// a `take` does.
///
/// That reading is sound today **because host work is projection-only by
/// construction** — `ordo_sema::Binder::host_open_of`: "a sealed field in
/// a filter, a sort key, or a group is `sealed_unsupported`". When sealed
/// filters land (F5), a paged plan whose sealed work sits in a filter
/// examines every candidate before the page can discard any, and this arm
/// must then additionally require the host work to be in the projection.
/// The same note already stands on `check_acknowledgments`' take-based
/// skip, for the same reason.
///
/// The page bounds only what reaches the page. A plan whose opened rows are
/// folded first (a `group`, a `reduce`, a `distinct`), or multiplied by a
/// many-side join, opens payloads for rows the page never sees, so it gets
/// no credit here ([`host_rows_are_page_rows`]).
///
/// The same reading is what `std-library.md` §5 rules 6 and 7 need: a
/// sealed-inclusive `std.E.list` decrypts through the normal path, and std
/// never registers an `unbounded-host` op.
///
/// Paging is read here and **not** folded into [`row_bound`], on purpose:
/// `refine_candidate_bound` reads that prover at a reduce's input, where no
/// page window exists, and the std-library suite pins that it stays out.
pub fn classify_with(q: &Query, schema: Option<&Schema>) -> CostClass {
    if query_has_host(q) {
        let bounded = match schema {
            Some(s) => matches!(row_bound(q, s), RowBound::AtMost(_)),
            None => false,
        } || (keyset_paged(q) && host_rows_are_page_rows(q));
        return if bounded {
            CostClass::BoundedHost
        } else {
            CostClass::UnboundedHost
        };
    }
    if query_has_blind(q) {
        CostClass::BlindIndex
    } else {
        CostClass::Sql
    }
}

/// Classification without a schema: cannot distinguish bounded from
/// unbounded host work, so it reports the conservative one.
pub fn classify(q: &Query) -> CostClass {
    classify_with(q, None)
}

/// True when the condition pins the anchor's id or a UNIQUE field to a
/// constant — DL's "identity/unique addressing" evidence. A blind-index
/// digest column stands in for its unique base field: `email$bidx = $1`
/// addresses at most one row exactly because `email` is unique.
fn pins_unique(
    c: &BCond,
    anchor_alias: &str,
    anchor_entity: Option<&str>,
    schema: &Schema,
) -> bool {
    match c {
        BCond::Cmp {
            op: ordo_syntax::CmpOp::Eq,
            lhs: BExpr::Col { alias, column, .. },
            rhs,
        } => {
            if alias != anchor_alias
                || !matches!(
                    rhs,
                    BExpr::Lit { .. }
                        | BExpr::Param { .. }
                        | BExpr::ByteaLit { .. }
                        // A digest site resolves to one constant, so it
                        // pins a unique column exactly as the digest it
                        // becomes does. Without this, moving the digest
                        // out of the artifact silently cost every
                        // blind-index lookup its row bound.
                        | BExpr::BlindDigest { .. }
                )
            {
                return false;
            }
            if column == "id" {
                return true;
            }
            let Some(entity) = anchor_entity.and_then(|e| schema.entity(e)) else {
                return false;
            };
            let base = column.strip_suffix("$bidx").unwrap_or(column);
            entity.field(base).map(|f| f.unique).unwrap_or(false)
        }
        BCond::And(v) => v
            .iter()
            .any(|i| pins_unique(i, anchor_alias, anchor_entity, schema)),
        _ => false,
    }
}

/// Evidence-based row bound (DL ~1840: identity/unique addressing, take n,
/// reduce-to-one; take[keys] deliberately absent — the ledger's item).
pub fn row_bound(q: &Query, schema: &Schema) -> RowBound {
    // `take $n` bounds at the PARAMETER'S DECLARED BOUND, not at a number
    // in the plan (DL "Other steps": the count may be a parameter, and the
    // parameter must carry a bound "so the op's cost class is still
    // statically provable"). The proof has two halves and neither stands
    // alone: `check_take_bounds` refuses an unbounded parameter here at
    // registration (`unbounded_take`), and the door refuses a value above
    // the bound at call time (`bound_violation`). A parameter with no
    // bound answers `None` and the plan is Unbounded — never quietly
    // bounded by a number the caller chooses.
    if let Some(n) = &q.limit {
        return match n.at_most() {
            Some(v) => RowBound::AtMost(v),
            None => RowBound::Unbounded,
        };
    }
    if let Some(g) = &q.group {
        if g.keys.is_empty() {
            return RowBound::AtMost(1);
        }
    }
    // Identity/unique evidence holds ONLY on a real table anchor with no
    // fanning source joins: on a Sub/Union anchor an output merely NAMED
    // `id` proves nothing (the audit's rename and union attacks), and a
    // source join multiplies the pinned row (customers(id == …) orders).
    // Dot-joins (q.joins) are many->one and safe.
    if let From::Table { entity, .. } = &q.from {
        if q.source_joins.is_empty() {
            let anchor = q.from.alias();
            if q.filters
                .iter()
                .any(|c| pins_unique(c, anchor, Some(entity.as_str()), schema))
            {
                return RowBound::AtMost(1);
            }
        }
    }
    if let From::Sub { query, .. } = &q.from {
        // A settled callee's bound survives the wrap when the caller adds
        // no multiplication (dot-joins are many->one).
        if let RowBound::AtMost(n) = row_bound(query, schema) {
            if q.source_joins.is_empty() {
                return RowBound::AtMost(n);
            }
        }
    }
    RowBound::Unbounded
}

/// **Is this plan a keyset page over a total order?** — the row-bound
/// evidence ruled in on 2026-09-05 (`docs/corpus-rulings.md`), and the
/// reason `std.E.list` derives.
///
/// The ruling admits paging as a bound under three conditions, and this
/// function is the second of them. In full:
///
/// 1. **The door pages every read.** `ordo_host` serves `OpKind::Read`
///    through `Executor::read_page_as` with a deployment-wide `page_size` —
///    a platform constant, never a caller's argument. That is the half a
///    compile-time check cannot see and must not pretend to; it is a
///    property of the door, asserted where the door is.
/// 2. **The plan has a TOTAL order** — this function. Keyset paging's whole
///    claim ("strictly after this row", never skipping, never repeating)
///    rests on the order being total; over a non-total order the seek
///    predicate silently drops the rows that tie with the cursor's row.
/// 3. **`take` is not a substitute and is not implied.** `plan_paged` puts
///    the take INSIDE the paged subquery, so a `take n` caps the whole
///    listing at n rather than windowing it. Nothing here adds one.
///
/// What makes an order total is the language's own tiebreak (R-C4/R-C7):
/// declared sorts, then the anchor row id — or, after a `group`, its keys;
/// after `distinct`, its outputs; after `append`, a per-arm row number. The
/// one shape that breaks it is a **many-side source join**: `customers[]
/// orders[]` repeats the anchor id once per order, so `id` ceases to
/// identify a row and the seek steps over the repeats. `row_bound` already
/// refuses its unique-pin evidence for the same reason and in the same
/// words ("a source join multiplies the pinned row"); this is that rule
/// asked about the ordering instead of about the pin.
///
/// An empty tiebreak is the keyless `reduce` — the one legitimately
/// unordered plan, and the one that needs no page because it is one row.
/// It answers `false` here and `AtMost(1)` from [`row_bound`], which is
/// also why `Executor::read_page_as`'s unpaged fallback for an `unpageable`
/// plan is sound rather than a hole.
pub fn keyset_paged(q: &Query) -> bool {
    if q.tiebreak.is_empty() {
        return false;
    }
    // A group's keys and a `distinct`'s outputs are unique per emitted row
    // by construction, so they order totally whatever the sources below
    // them did. Nothing under them needs re-examining.
    if q.group.as_ref().is_some_and(|g| !g.keys.is_empty()) || q.distinct {
        return true;
    }
    // Dot-joins (`q.joins`) are many->one and cannot multiply a row.
    if q.source_joins.iter().any(|j| j.many_side) {
        return false;
    }
    match &q.from {
        // `append`'s tiebreak ends in a per-arm row number precisely so the
        // whole order is total rather than total-up-to-identity, so the
        // arms' own shapes cannot cost it.
        From::Union { .. } => true,
        // The outer tiebreak is the callee's, rebased onto the wrapper's
        // columns: it is total exactly when the callee's was.
        From::Sub { query, .. } => keyset_paged(query),
        From::CteRef { name, .. } => q
            .ctes
            .iter()
            .find(|c| &c.name == name)
            .is_some_and(|c| keyset_paged(&c.query)),
        From::Table { .. } => true,
    }
}

// ── the request-time half: effective cost class per request (§7.1) ──────
//
// A registered class describes the op AS WRITTEN. A caller's `$refine`
// changes the work the op actually does, so the served class is recomputed
// per request — `api-engine-design.md` §7.1 and `std-library.md` §2.1 rule 4,
// both [settled]. The three functions below are that computation, and they
// live here rather than at the door for one reason: the door must not own a
// second cost model. Registration and the request-time gate now read from
// one `CostClass`, one `RowBound`, and one lattice.

/// The set a **post-everything** refinement is applied to.
///
/// A host-side condition (a sealed field with no blind index) lands after
/// the op's own `take` (§7; open and blind-index conditions compile into
/// the plan since 2026-10-05 and are `sql` or `blind-index` whatever this
/// says), so for a row-shaped plan the set it filters is the plan's own
/// result and its bound is the plan's own [`row_bound`]. §7.1 turns on the
/// one shape where that is not the whole story:
///
/// > Where the gate still bites is the shape that has no page window: an op
/// > whose result is statically one row — a `reduce`, and `std.E.count`
/// > under the list-manifest pairing — refines *before* the reduce, over the
/// > op's whole result set.
///
/// `row_bound` answers `1` for a keyless reduce, and it is right: one row is
/// what the op RETURNS. The refinement, though, is applied to what the
/// reduce EXAMINES, and that set is bounded by nothing the registration gate
/// looked at — which is exactly the gap a `count` op walks through. So this
/// asks the bound question at the reduce's input.
///
/// The distinction is the same one [`check_venue`] makes about `take`:
/// returned rows and examined rows are different quantities, and a gate that
/// confuses them is a gate you can step around.
pub fn refine_candidate_bound(q: &Query, schema: &Schema) -> RowBound {
    if let Some(g) = &q.group {
        if g.keys.is_empty() {
            // The reduce's input, with the reduce removed. `having` goes
            // with it — a bucket filter has no meaning without buckets —
            // and everything else (source, filters, joins, limit) is the
            // evidence `row_bound` weighs, unchanged.
            let mut input = q.clone();
            input.group = None;
            input.having.clear();
            return row_bound(&input, schema);
        }
    }
    row_bound(q, schema)
}

/// The class a **refinement condition on one field** implies (§7.1's
/// `class_of`).
///
/// - An **open** field costs `sql`: the condition is an ordinary predicate
///   over values the plan already produced, and it changes no venue. This is
///   the direction that must not over-refuse — a false positive here breaks
///   every refining caller, and refinement is the whole findBy replacement.
/// - A **sealed** field raises the class, and how far is the blind index's
///   answer. `sealed indexed` equality rides the digest — one HMAC compare
///   in SQL, which is what `blind-index` names.
/// - A sealed field with **no** blind index cannot be answered in the
///   database at all: every candidate payload is opened host-side. Bounded
///   or unbounded is then [`refine_candidate_bound`]'s question, asked at
///   the venue exactly as [`classify_with`] asks it.
///
/// `equality` is the caller's operator being `eq`/`in`. It is a parameter
/// rather than an assumption because a non-equality sealed condition is
/// refused at the door (`std-library.md` §2.1 rule 2) and this function must
/// still answer honestly if it is ever asked out of that order: an ordering
/// comparison over sealed values is host work, index or no index.
pub fn refine_field_class(
    sealed: bool,
    blind_indexed: bool,
    equality: bool,
    candidate: RowBound,
) -> CostClass {
    if !sealed {
        return CostClass::Sql;
    }
    if blind_indexed && equality {
        return CostClass::BlindIndex;
    }
    match candidate {
        RowBound::AtMost(_) => CostClass::BoundedHost,
        RowBound::Unbounded => CostClass::UnboundedHost,
    }
}

/// `effective = registered ⊔ class_of(refinement fields, refinement ops)`
/// — §7.1's formula, with the per-field classes already computed by
/// [`refine_field_class`].
///
/// Only `where` fields are joined. `pick` names fields too, but a `pick`
/// removes columns from a result the op already produces: a sealed field in
/// the manifest is one the plan already opens, so narrowing to it costs
/// nothing new and dropping it saves work. Refinement narrows results; only
/// a condition can change what has to be examined to produce them.
pub fn effective_class(
    registered: CostClass,
    conditions: impl IntoIterator<Item = CostClass>,
) -> CostClass {
    conditions
        .into_iter()
        .fold(registered, |acc, c| acc.join(c))
}

/// EVERY registration gate in one call — the viral acknowledgment rule,
/// the row-cardinality prover, and the write rules. Embedders must use
/// this, not the halves: the audit showed the viral rule's self-bounded
/// skip is only sound when the row gate runs beside it, and a write op
/// that no data can make valid must fail here for the same reason a read
/// op that no data can bound does.
pub fn check_registration(schema: &Schema, registry: &Registry) -> Result<(), SemaError> {
    registry.check_reserved_params()?;
    registry.check_content_params()?;
    registry.check_param_places()?;
    registry.check_filled_types()?;
    registry.check_take_bounds()?;
    registry.check_reproducible()?;
    registry.check_acknowledgments()?;
    check_unguarded(schema, registry)?;
    check_venue(schema, registry)?;
    check_row_bounds(schema, registry)?;
    // The public gate (`plan-syntax-additions.md` §1): what a public op may
    // disclose is proved here, so the public path can skip the guard walk
    // and the audit row at run time.
    crate::public::check_public(schema, registry)?;
    // A person's own download (owner ruling 2026-10-22): a marked op must be
    // a read the door can run with no parameters, over one entity.
    crate::download::check_download(schema, registry)?;
    // The release rule (27aq) binds through the guard compiler once, here,
    // so a walk it refuses fails the deploy and not the first leave.
    ordo_sema::check_release(schema)?;
    // The write half (DL §6 rules 2 and 3): read gates bound work, these
    // bound what a statement is allowed to mean. Same gate, because a
    // write op that cannot be insert-complete must not deploy either.
    crate::writes::check_writes(schema, registry)?;
    check_routines(schema, registry)
}

/// A routine's body is a write group, and it passes the gate a write op
/// passes (checklist 12a). Checked in a registry of its own, so the routine
/// is never registered where a caller could reach it.
///
/// An `each` routine's pipeline is bound here too, and passes the read
/// half of the gate as a read op would, apart from the row bound: the
/// per-run row limit bounds what one run reads. Its emission is checked
/// with each row name as a typed parameter (`crate::each::check_op`).
pub fn check_routines(schema: &Schema, registry: &Registry) -> Result<(), SemaError> {
    if schema.routines.is_empty() {
        return Ok(());
    }
    let mut ops = Vec::new();
    for r in &schema.routines {
        match crate::each::read_op(r) {
            None => ops.push(ordo_schema::routine::routine_op(r)),
            Some(read) => {
                let mut reads = registry.clone();
                reads.extend_from(&ordo_syntax::Program {
                    ops: vec![read],
                    ..ordo_syntax::Program::default()
                });
                reads.check_reserved_params()?;
                reads.check_reproducible()?;
                reads.check_acknowledgments()?;
                ops.push(crate::each::check_op(schema, registry, r)?);
            }
        }
    }
    let mut with = registry.clone();
    with.extend_from(&ordo_syntax::Program {
        ops,
        ..ordo_syntax::Program::default()
    });
    with.check_reserved_params()?;
    with.check_param_places()?;
    with.check_reproducible()?;
    with.check_acknowledgments()?;
    crate::writes::check_writes(schema, &with)
}

/// The venue gate (DL ~1816, the landmine ledgered when host classes were
/// unreachable and now live): **`take` bounds RETURNED rows, not EXAMINED
/// ones.** An op that opens sealed payloads does work per row the database
/// hands over, and a trailing `take 50` does not reduce that work — it
/// only hides it from the row bound.
///
/// So an unbounded-host op needs the `unbounded` acknowledgment even when
/// its row bound is small, and the acknowledgment is keyed on the VENUE
/// class rather than on the returned-row count. Without this the
/// self-bounded skip in the row gate generalizes to exactly the shape DL
/// rejects.
pub fn check_venue(schema: &Schema, registry: &Registry) -> Result<(), SemaError> {
    let mut names: Vec<&String> = registry.names().collect();
    names.sort();
    for name in names {
        let Some(op) = registry.get(name) else {
            continue;
        };
        if op.flags.unbounded || op.read_pipeline().is_none() {
            continue;
        }
        let q = bind_op_generic(schema, registry, name, ordo_sema::Clock::CORPUS)?;
        if classify_with(&q, Some(schema)) == CostClass::UnboundedHost {
            return Err(SemaError {
                code: "unacknowledged_unbounded",
                message: format!(
                    "`{name}` opens sealed payloads for an unbounded number of rows — host \
                     work grows with the space, and a `take` bounds what is RETURNED, not \
                     what is EXAMINED; acknowledge with `op {name} unbounded = …`"
                ),
                span: op.name.span,
            });
        }
    }
    Ok(())
}

/// R-G8's two compile checks. The `unguarded` flag must be load-bearing:
/// legal only where the bypass is live — a guarded entity sourced directly
/// in the op's own body, or a composed callee whose own bypass is live
/// (GC035's inherited acknowledgment). A flag stripping nothing is
/// `unguarded_superfluous` (GC032/GC033); composing a live bypass WITHOUT
/// the flag is `unacknowledged_unguarded` (GC034) — the same viral shape
/// as `unbounded`, for the same reason: the caller's manifest must
/// disclose what its results bypass.
pub fn check_unguarded(schema: &Schema, registry: &Registry) -> Result<(), SemaError> {
    // A guard body `open` bypasses nothing; only walk-guarded entities count.
    let guarded = |name: &str| {
        schema.entity(name).is_some_and(|e| {
            e.guards
                .as_ref()
                .is_some_and(|g| matches!(g.read, ordo_schema::GuardBody::Walk(_)))
        })
    };
    // Does this op's own body directly source a guarded entity?
    fn sources_of(pl: &ordo_syntax::Pipeline, out: &mut Vec<String>) {
        for l in &pl.lets {
            sources_of(&l.pipeline, out);
        }
        for s in &pl.sources {
            out.push(s.name.name.clone());
        }
        for st in &pl.steps {
            match st {
                ordo_syntax::Step::Match {
                    kind:
                        ordo_syntax::decl::MatchKind::Any(s) | ordo_syntax::decl::MatchKind::None(s),
                    ..
                }
                | ordo_syntax::Step::Append { source: s, .. } => out.push(s.name.name.clone()),
                _ => {}
            }
        }
    }
    // Is `name`'s bypass live? (its flag strips a real guard, directly or
    // through a composed live bypass — acyclic by the cycle gate.)
    fn live(registry: &Registry, guarded: &dyn Fn(&str) -> bool, name: &str) -> bool {
        let Some(op) = registry.get(name) else {
            return false;
        };
        if !op.flags.unguarded {
            return false;
        }
        let Some(pl) = op.read_pipeline() else {
            return false;
        };
        let mut srcs = Vec::new();
        sources_of(pl, &mut srcs);
        srcs.iter()
            .any(|s| guarded(s) || live(registry, guarded, s))
    }

    let mut names: Vec<&String> = registry.names().collect();
    names.sort();
    for name in names {
        let Some(op) = registry.get(name) else {
            continue;
        };
        let Some(pl) = op.read_pipeline() else {
            continue;
        };
        let mut srcs = Vec::new();
        sources_of(pl, &mut srcs);
        let direct = srcs.iter().any(|s| guarded(s));
        let composed_live = srcs
            .iter()
            .any(|s| !guarded(s) && live(registry, &guarded, s));
        if op.flags.unguarded && !direct && !composed_live {
            return Err(SemaError {
                code: "unguarded_superfluous",
                message: format!(
                    "`{name}` is marked unguarded but its own body sources no guarded entity and inherits no live bypass — the flag would be decoration (R-G8)"
                ),
                span: op.name.span,
            });
        }
        if !op.flags.unguarded && composed_live {
            return Err(SemaError {
                code: "unacknowledged_unguarded",
                message: format!(
                    "`{name}` composes a callee whose guard bypass is live; carry the acknowledgment: `op {name} unguarded = …` (R-G8)"
                ),
                span: op.name.span,
            });
        }
    }
    Ok(())
}

/// The registration gate, row-cardinality half (DL "the unbounded gate
/// covers row cardinality, not just venue" [settled]): every registered op
/// either proves a bound, **is served one keyset page at a time**, or
/// carries `unbounded`. `unbounded-host` and unbounded-`sql` are the same
/// acknowledgment word.
///
/// The middle clause is the 2026-09-05 ruling ([`keyset_paged`]). It is
/// this gate's alone, on purpose: [`classify_with`] and
/// [`refine_candidate_bound`] keep asking the strict [`row_bound`], because
/// the quantity they need bounded is not the quantity a page bounds — host
/// staging opens what the plan EXAMINES, and a refinement under `count`
/// runs before the reduce, where there is no page window at all. Widening
/// the prover instead of the gate would have reopened exactly the sealed-
/// refinement oracle §2.1 rule 3 exists to close.
pub fn check_row_bounds(schema: &Schema, registry: &Registry) -> Result<(), SemaError> {
    let mut names: Vec<&String> = registry.names().collect();
    names.sort();
    for name in names {
        let Some(op) = registry.get(name) else {
            continue;
        };
        if op.flags.unbounded {
            continue;
        }
        // Write ops have no read pipeline to bound; their row-work story
        // (statement ranges) is the write protocol's own gate, P5.
        if op.read_pipeline().is_none() {
            continue;
        }
        let q = bind_op_generic(schema, registry, name, ordo_sema::Clock::CORPUS)?;
        if row_bound(&q, schema) == RowBound::Unbounded && !keyset_paged(&q) {
            let span = registry
                .get(name)
                .map(|o| o.name.span)
                .unwrap_or_else(|| ordo_syntax::Span::new(0, 0, 1, 1));
            return Err(SemaError {
                code: "unacknowledged_unbounded",
                message: format!(
                    "`{name}` has no provable row bound and cannot be keyset-paged — its order is not total, so paging would skip rows; acknowledge with `op {name} unbounded = …` or bound it"
                ),
                span,
            });
        }
    }
    Ok(())
}
