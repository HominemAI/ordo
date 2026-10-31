//! **A sub-select keeps only the columns its outer query reads** (reported
//! fix 25v, `filtered-term-reach`).
//!
//! A filtered join term is lowered to a sub-select, and the sub-select
//! carries every open field of its entity so that the list's projection,
//! and any later step, can read through it (fix 25s). A wrapped callee
//! carries every output of the callee for the same reason. Nothing trimmed
//! them afterwards, so a caller that dropped a column still selected it
//! below. The cost class, the reach and the release all read the bound
//! query, so they over-reported: `memos[vname, pname]` over a filtered term
//! carrying a sealed `secret` read `unbounded-host` and reached every field
//! of the term's entity, where the plain join read `sql`.
//!
//! This pass runs once, after the whole pipeline is bound and every callee
//! is composed or wrapped. For each `From::Sub` (an anchor or a join
//! target) it drops the outputs no expression outside the sub-select
//! references, with their `host_open` entries. It repeats until nothing
//! changes, because dropping an output can free a column one level down.
//!
//! What it never drops:
//!
//! - an output of a `distinct` sub-select, whose columns decide which rows
//!   are equal;
//! - `id` and the binder's hidden `$`-names (a wrapped callee's tiebreak),
//!   which the row key of a synced read may follow;
//! - the last output, so a sub-select always selects something.
//!
//! A union arm's outputs are matched by name across arms and are left
//! alone. A `let`'s CTE is left alone too: it is read by name and its
//! tiebreak names every column anyway.

use std::collections::HashSet;

use crate::ir::{BCond, BExpr, From, Query, Sort};

type Used = HashSet<(String, String)>;

/// Prune every sub-select in `q` to the columns read from outside it.
pub(crate) fn prune_subs(q: &mut Query) {
    // Each round drops at least one output or stops, so the outputs bound
    // the loop. The cap only guards against a mistake in that reasoning.
    for _ in 0..10_000 {
        let mut used = Used::new();
        query_refs(q, &mut used);
        if !prune_query(q, &used) {
            return;
        }
    }
}

// ── what is read ─────────────────────────────────────────────────────────

fn query_refs(q: &Query, used: &mut Used) {
    from_refs(&q.from, used);
    for j in &q.joins {
        used.insert((j.from_alias.clone(), j.from_column.clone()));
        if let Some(g) = &j.guard {
            cond_refs(g, used);
        }
    }
    for j in &q.source_joins {
        from_refs(&j.from, used);
        for (a, b) in &j.on {
            expr_refs(a, used);
            expr_refs(b, used);
        }
        if let Some(g) = &j.guard {
            cond_refs(g, used);
        }
    }
    for c in q.filters.iter().chain(&q.having) {
        cond_refs(c, used);
    }
    if let Some(g) = &q.group {
        for k in &g.keys {
            expr_refs(k, used);
        }
    }
    for o in &q.outputs {
        expr_refs(&o.expr, used);
    }
    sorts_refs(&q.sort, used);
    for t in &q.tiebreak {
        expr_refs(t, used);
    }
    for c in &q.ctes {
        query_refs(&c.query, used);
    }
}

fn from_refs(f: &From, used: &mut Used) {
    match f {
        From::Sub { query, .. } => query_refs(query, used),
        From::Union { arms, .. } => {
            for a in arms {
                query_refs(a, used);
            }
        }
        From::Table { .. } | From::CteRef { .. } => {}
    }
}

fn sorts_refs(s: &[Sort], used: &mut Used) {
    for k in s {
        expr_refs(&k.expr, used);
    }
}

fn expr_refs(e: &BExpr, used: &mut Used) {
    match e {
        BExpr::Col { alias, column, .. } => {
            used.insert((alias.clone(), column.clone()));
        }
        BExpr::Neg { expr, .. } => expr_refs(expr, used),
        BExpr::DocPath { doc, .. } => expr_refs(doc, used),
        BExpr::Binary { lhs, rhs, .. } => {
            expr_refs(lhs, used);
            expr_refs(rhs, used);
        }
        BExpr::Fn { args, .. } => {
            for a in args {
                expr_refs(a, used);
            }
        }
        BExpr::Cond { arms, else_, .. } => {
            for (c, v) in arms {
                cond_refs(c, used);
                expr_refs(v, used);
            }
            expr_refs(else_, used);
        }
        BExpr::Agg {
            arg,
            arg2,
            order,
            mask,
            ..
        } => {
            for a in [arg, arg2].into_iter().flatten() {
                expr_refs(a, used);
            }
            sorts_refs(order, used);
            if let Some(m) = mask {
                cond_refs(m, used);
            }
        }
        BExpr::RowNumber {
            partition, order, ..
        } => {
            for p in partition {
                expr_refs(p, used);
            }
            sorts_refs(order, used);
        }
        BExpr::Lit { .. }
        | BExpr::Param { .. }
        | BExpr::BlindDigest { .. }
        | BExpr::ByteaLit { .. }
        | BExpr::ScalarSub { .. } => {}
    }
}

fn cond_refs(c: &BCond, used: &mut Used) {
    match c {
        BCond::Cmp { lhs, rhs, .. } | BCond::Text { lhs, rhs, .. } => {
            expr_refs(lhs, used);
            expr_refs(rhs, used);
        }
        BCond::In { expr, list } => {
            expr_refs(expr, used);
            for e in list {
                expr_refs(e, used);
            }
        }
        BCond::IsNull { expr, .. } | BCond::Bare(expr) => expr_refs(expr, used),
        BCond::And(cs) | BCond::Or(cs) => {
            for c in cs {
                cond_refs(c, used);
            }
        }
        BCond::Not(c) => cond_refs(c, used),
        BCond::Exists { query, .. } => query_refs(query, used),
        BCond::TreeWalk { expr, anchor, .. } => {
            expr_refs(expr, used);
            expr_refs(anchor, used);
        }
    }
}

// ── dropping what is not ─────────────────────────────────────────────────

/// Prune the sub-selects this query and everything under it owns. True
/// when any output was dropped.
fn prune_query(q: &mut Query, used: &Used) -> bool {
    let mut changed = prune_from(&mut q.from, used);
    for j in &mut q.source_joins {
        changed |= prune_from(&mut j.from, used);
        if let Some(g) = &mut j.guard {
            changed |= prune_cond(g, used);
        }
    }
    for j in &mut q.joins {
        if let Some(g) = &mut j.guard {
            changed |= prune_cond(g, used);
        }
    }
    for c in q.filters.iter_mut().chain(q.having.iter_mut()) {
        changed |= prune_cond(c, used);
    }
    for c in &mut q.ctes {
        changed |= prune_query(&mut c.query, used);
    }
    changed
}

fn prune_from(f: &mut From, used: &Used) -> bool {
    match f {
        From::Sub { query, alias } => {
            let mut changed = trim(query, alias, used);
            changed |= prune_query(query, used);
            changed
        }
        From::Union { arms, .. } => {
            let mut changed = false;
            for a in arms {
                changed |= prune_query(a, used);
            }
            changed
        }
        From::Table { .. } | From::CteRef { .. } => false,
    }
}

fn prune_cond(c: &mut BCond, used: &Used) -> bool {
    match c {
        BCond::And(cs) | BCond::Or(cs) => {
            let mut changed = false;
            for c in cs {
                changed |= prune_cond(c, used);
            }
            changed
        }
        BCond::Not(c) => prune_cond(c, used),
        BCond::Exists { query, .. } => prune_query(query, used),
        _ => false,
    }
}

/// Drop the outputs of the sub-select under `alias` that nothing outside
/// it reads, with their obligations.
fn trim(query: &mut Query, alias: &str, used: &Used) -> bool {
    if query.distinct {
        return false;
    }
    let read = |name: &str| {
        name == "id" || name.starts_with('$') || used.contains(&(alias.to_string(), name.into()))
    };
    let keep: Vec<bool> = query.outputs.iter().map(|o| read(&o.name)).collect();
    if keep.iter().all(|k| *k) {
        return false;
    }
    if !keep.iter().any(|k| *k) {
        // Keep one column: a select of nothing is not worth the dialect
        // question. An open one, so the sub-select opens nothing.
        if query.outputs.len() <= 1 {
            return false;
        }
        let sealed = |n: &str| query.host_open.iter().any(|(h, _, _)| h == n);
        let at = query
            .outputs
            .iter()
            .position(|o| !sealed(&o.name))
            .unwrap_or(0);
        let one = query.outputs.swap_remove(at);
        query.outputs = vec![one];
    } else {
        let mut i = 0;
        query.outputs.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
    }
    let outputs = &query.outputs;
    query
        .host_open
        .retain(|(n, _, _)| outputs.iter().any(|o| &o.name == n));
    true
}
