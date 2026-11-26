//! Peer entities served through the holder's door
//! (`plan-federation-grants.md` §4, step G5).
//!
//! An external entity declared from a peer whose `ORDO_PEER_<NAME>_URL` is
//! set is never read from the peer's tables. Each read that touches one is
//! answered in three moves, inside one call:
//!
//! 1. **Ask the holder.** Each alias of a peer entity in the bound query is
//!    one call to the holder's `std.<entity>.list`, with what the read says
//!    about it: its conditions as `$refine.where`, its fields as `pick`,
//!    and, for a read whose rows are the peer's rows, its order and
//!    `take` as `sort` and `limit`. The holder runs that as its own SQL,
//!    under its own rules for the person.
//! 2. **Join in one batch.** A peer entity reached through a native ref
//!    (`comment.note.title`, a guard that walks into it) is one call with
//!    `id in [...]`, the refs of the rows read here: never one call per row.
//!    The refs come from the same read run once with the peer's rows empty
//!    (the probe), which a left join of a ref leaves unchanged.
//! 3. **Run the read here** over those rows, served to the statement as
//!    typed literals (`ordo_plan::external_source`). They live for that
//!    statement only: no copy of the holder's data outlives the call
//!    (owner: "no copy. data lives within its deployment end of story").
//!
//! `$caller` on a peer entity is the holder's: `field == $caller` goes to
//! the holder as a `caller` condition, decided against its own member for
//! the person the call is made for. Here it then reads "the holder found
//! one", which every served row satisfies. A `$caller` the holder cannot be
//! sent (under `or` or `not`) is refused, naming the fix.
//!
//! **Consistency.** There is no transaction across deployments. 2 reads of
//! the holder in one op are 2 calls and not one snapshot, and the rows
//! read here are not of the same instant as the holder's.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ordo_sema::ir::{BCond, BExpr, ExternalRead, From as Src, Query, ServedRows};
use ordo_syntax::{CmpOp, Expr, ScalarTy};
use serde_json::{json, Map, Value};

use crate::{ExecFail, Pool};

/// One call to a holder: `std.<entity>.list` with a refinement.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetch {
    /// The `peer` nickname.
    pub peer: String,
    /// The holder's entity: the call is `std.<entity>.list`.
    pub entity: String,
    /// The `$refine.where` conditions, in the door's shape.
    pub conds: Vec<Value>,
    /// The holder's fields this read needs.
    pub pick: Vec<String>,
    /// `(field, desc)`, when the read's order is sent.
    pub sort: Vec<(String, bool)>,
    /// The rows of the one answer, when the read's `take` is sent.
    pub limit: Option<usize>,
    /// Read every page the holder has: nothing bounds the read in one
    /// answer.
    pub whole: bool,
}

impl Fetch {
    /// The `$refine` envelope for it.
    pub fn refine(&self) -> Value {
        let mut r = Map::new();
        r.insert("where".into(), Value::Array(self.conds.clone()));
        r.insert("pick".into(), json!(self.pick));
        if !self.sort.is_empty() {
            r.insert(
                "sort".into(),
                Value::Array(
                    self.sort
                        .iter()
                        .map(|(f, d)| json!({ "field": f, "desc": d }))
                        .collect(),
                ),
            );
        }
        if let Some(n) = self.limit {
            r.insert("limit".into(), json!(n));
        }
        Value::Object(r)
    }
}

/// Why a holder was not read. Its code is the door's class:
/// `peer_unavailable` (retry) or `peer_refused` (the holder or this
/// deployment refused it as written; the message names the fix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerReadFail {
    pub code: &'static str,
    pub message: String,
}

impl PeerReadFail {
    pub const UNAVAILABLE: &'static str = "peer_unavailable";
    pub const REFUSED: &'static str = "peer_refused";

    pub fn refused(message: impl Into<String>) -> PeerReadFail {
        PeerReadFail {
            code: Self::REFUSED,
            message: message.into(),
        }
    }
}

impl From<PeerReadFail> for ExecFail {
    fn from(f: PeerReadFail) -> ExecFail {
        ExecFail::Peer {
            code: f.code,
            message: f.message,
        }
    }
}

/// The host's half: the one client to other deployments' doors
/// (`ordo_host::peer_client`). `caller` is the member this read is made
/// for, `None` when nobody is signed in: then the call is made as this
/// deployment itself.
#[async_trait::async_trait]
pub trait PeerFetch: Send + Sync {
    async fn fetch(
        &self,
        pool: &Pool,
        caller: Option<&str>,
        fetch: &Fetch,
    ) -> Result<Vec<Map<String, Value>>, PeerReadFail>;

    /// The most refs one batched call carries; more are split into calls
    /// of this size.
    fn batch_keys(&self) -> usize;
}

/// How a read is paged, so a probe reads the same rows.
#[derive(Debug, Clone, Copy)]
pub struct Paging<'a> {
    pub page_size: i64,
    pub after: Option<&'a [Option<String>]>,
}

/// Where a peer alias sits in the read.
#[derive(Debug, Clone, PartialEq)]
enum Place {
    /// The outermost level's source.
    Root,
    /// A ref hop of the outermost level: `from_alias.from_column` holds its
    /// id.
    Joined {
        from_alias: String,
        from_column: String,
    },
    /// Anywhere else: a nested level, a sub-select, a multi-source join.
    Other,
}

#[derive(Debug, Clone)]
struct Site {
    alias: String,
    read: ExternalRead,
    place: Place,
    /// The conditions every row of this alias must meet: the top-level
    /// conjuncts of its level that read only it, and its join guard's.
    own: Vec<BCond>,
    /// `$caller` conditions on it in a place where they bound every row it
    /// gives the read: its own conditions, or a top-level conjunct of its
    /// level. Each is sent as a `caller` condition.
    callers: BTreeSet<String>,
}

/// Serve every door-read peer alias of `q` before it is planned. A read
/// with none returns at once and costs nothing. `run` runs the probe:
/// paged as the read is, or whole when its second argument is true (a
/// filter that read the peer was dropped from it, so the read's page no
/// longer bounds it).
pub async fn serve<R, Fut>(
    q: &mut Query,
    fetcher: &dyn PeerFetch,
    pool: &Pool,
    caller: Option<&str>,
    paging: Option<Paging<'_>>,
    run: R,
) -> Result<(), ExecFail>
where
    R: FnOnce(Query, bool) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<Vec<Option<String>>>, ExecFail>>,
{
    let Some(externals) = externals_of(q) else {
        return Ok(());
    };
    if !externals.values().any(|r| r.door.is_some()) {
        return Ok(());
    }
    let mut sites = Vec::new();
    collect(q, &externals, true, &mut sites);
    if sites.is_empty() {
        return Ok(());
    }
    // Every `$caller` on a door alias must be one the holder can be sent.
    check_callers(q, &sites)?;

    let person = caller.filter(|c| *c != ANONYMOUS);
    let mut served: BTreeMap<String, Vec<Vec<Option<String>>>> = BTreeMap::new();

    // The outermost read's own rows, when they are the peer's: its order
    // and `take` are sent too, so the holder answers exactly the page.
    let (level, wrapped) = root_level(q);
    // Past a wrapper, the level's own `take` settles its rows whatever
    // page the wrapper reads; with none, the page and the cursor do.
    let paging = if wrapped { None } else { paging };
    let root_whole = root_is_whole(level, &sites, paging);
    let window = sites
        .iter()
        .find(|s| s.place == Place::Root)
        .and_then(|s| root_window(level, s, paging));
    let keyed: Vec<Site> = sites
        .iter()
        .filter(|s| matches!(s.place, Place::Joined { .. }))
        .cloned()
        .collect();

    for s in sites
        .iter()
        .filter(|s| !matches!(s.place, Place::Joined { .. }))
    {
        if !s.callers.is_empty() && person.is_none() {
            // `$caller` with nobody signed in matches nothing (DL `peer`).
            served.insert(s.alias.clone(), Vec::new());
            continue;
        }
        let mut f = base_fetch(s);
        if s.place == Place::Root && !root_whole {
            if let Some((sort, limit)) = window.clone() {
                f.sort = sort;
                f.limit = Some(limit);
                f.whole = false;
            }
        }
        let rows = fetcher.fetch(pool, person, &f).await?;
        served.insert(s.alias.clone(), to_rows(&s.read, &rows)?);
    }

    if !keyed.is_empty() {
        // The probe: the same read with each joined peer's rows empty,
        // which leaves the native rows as they are, and their refs out.
        let mut probe = q.clone();
        let keyed_aliases: BTreeSet<String> = keyed.iter().map(|s| s.alias.clone()).collect();
        let dropped = {
            let level = root_level_mut(&mut probe);
            let dropped = drop_filters_on(level, &keyed_aliases);
            if dropped {
                // A filter that read the peer no longer narrows the probe,
                // so its `take` no longer bounds it either.
                level.limit = None;
            }
            dropped
        };
        let mut probe_served = served.clone();
        for s in &keyed {
            probe_served.insert(s.alias.clone(), Vec::new());
        }
        rewrite_callers(&mut probe, &sites);
        let first = probe.outputs.len();
        for s in &keyed {
            let Place::Joined {
                from_alias,
                from_column,
            } = &s.place
            else {
                continue;
            };
            push_output(
                &mut probe,
                &format!("$peerref_{}", s.alias),
                BExpr::Col {
                    alias: from_alias.clone(),
                    column: from_column.clone(),
                    ty: ScalarTy::Text,
                },
            );
        }
        set_served(&mut probe, &externals, probe_served);
        let rows = run(probe, dropped).await?;
        for (i, s) in keyed.iter().enumerate() {
            if !s.callers.is_empty() && person.is_none() {
                served.insert(s.alias.clone(), Vec::new());
                continue;
            }
            let mut keys: Vec<Value> = Vec::new();
            let mut seen = BTreeSet::new();
            for row in &rows {
                let Some(Some(id)) = row.get(first + i) else {
                    continue;
                };
                let Some(raw) = ordo_plan::external_key_of(id, &s.read) else {
                    continue;
                };
                if seen.insert(raw.to_string()) {
                    keys.push(key_value(&s.read, raw));
                }
            }
            let mut got = Vec::new();
            let size = fetcher.batch_keys().max(1);
            for chunk in keys.chunks(size) {
                let mut f = base_fetch(s);
                f.conds.push(json!({
                    "field": s.read.key_column,
                    "op": "in",
                    "value": chunk,
                }));
                f.limit = Some(chunk.len());
                f.whole = false;
                got.extend(fetcher.fetch(pool, person, &f).await?);
            }
            served.insert(s.alias.clone(), to_rows(&s.read, &got)?);
        }
    }

    rewrite_callers(q, &sites);
    set_served(q, &externals, served);
    Ok(())
}

/// The member id an anonymous call carries (`ordo_api::ANONYMOUS`).
const ANONYMOUS: &str = "$anonymous";

fn externals_of(q: &Query) -> Option<ordo_sema::ir::ExternalReads> {
    if q.externals.is_some() {
        return q.externals.clone();
    }
    match &q.from {
        Src::Sub { query, .. } => externals_of(query),
        Src::Union { arms, .. } => arms.iter().find_map(externals_of),
        _ => None,
    }
    .or_else(|| q.ctes.iter().find_map(|c| externals_of(&c.query)))
}

/// Each condition of a conjunction, flattened.
fn conjuncts(cs: &[BCond]) -> Vec<&BCond> {
    let mut out = Vec::new();
    for c in cs {
        match c {
            BCond::And(items) => out.extend(conjuncts(items)),
            other => out.push(other),
        }
    }
    out
}

fn collect(
    q: &Query,
    externals: &ordo_sema::ir::ExternalReads,
    outermost: bool,
    out: &mut Vec<Site>,
) {
    let door = |entity: &str| externals.get(entity).filter(|r| r.door.is_some()).cloned();
    let level = conjuncts(&q.filters);
    // A joined alias is null where its row was not served, so of the
    // level's conditions on it only those false on null may narrow what the
    // holder sends: `note.title != "x"` holds for a row it did not send.
    // Its join guard is in the `ON`, and narrows like any guard.
    let site = |alias: &str, read: ExternalRead, place: Place, guard: Option<&BCond>| {
        let joined = !is_level_source(q, alias);
        let mut own: Vec<BCond> = level
            .iter()
            .filter(|c| only_reads(c, alias))
            .filter(|c| !joined || false_on_null(c))
            .map(|c| (*c).clone())
            .collect();
        if let Some(g) = guard {
            own.extend(conjuncts(std::slice::from_ref(g)).into_iter().cloned());
        }
        let mut callers = BTreeSet::new();
        for c in own.iter().chain(level.iter().copied()) {
            if let Some(col) = caller_on(c, alias) {
                callers.insert(col);
            }
        }
        Site {
            alias: alias.to_string(),
            read,
            place,
            own,
            callers,
        }
    };
    match &q.from {
        Src::Table { entity, alias } => {
            if let Some(r) = door(entity) {
                let place = if outermost { Place::Root } else { Place::Other };
                out.push(site(alias, r, place, None));
            }
        }
        // A call the binder wrapped (its `take` settles its rows first) is
        // still the read's own level when the wrapper only pages it.
        Src::Sub { query, .. } => collect(query, externals, outermost && passes(q), out),
        Src::Union { arms, .. } => arms.iter().for_each(|a| collect(a, externals, false, out)),
        Src::CteRef { .. } => {}
    }
    for j in &q.joins {
        if let Some(r) = door(&j.table) {
            let from_is_door = out.iter().any(|s| s.alias == j.from_alias);
            let place = if outermost && !from_is_door && !q.distinct && q.group.is_none() {
                Place::Joined {
                    from_alias: j.from_alias.clone(),
                    from_column: j.from_column.clone(),
                }
            } else {
                Place::Other
            };
            out.push(site(&j.alias, r, place, j.guard.as_ref()));
        }
        if let Some(g) = &j.guard {
            nested_in_cond(g, externals, out);
        }
    }
    for sj in &q.source_joins {
        match &sj.from {
            Src::Table { entity, alias } => {
                if let Some(r) = door(entity) {
                    out.push(site(alias, r, Place::Other, sj.guard.as_ref()));
                }
            }
            Src::Sub { query, .. } => collect(query, externals, false, out),
            _ => {}
        }
        if let Some(g) = &sj.guard {
            nested_in_cond(g, externals, out);
        }
    }
    for c in q.filters.iter().chain(&q.having) {
        nested_in_cond(c, externals, out);
    }
    for c in &q.ctes {
        collect(&c.query, externals, false, out);
    }
}

/// Is `alias` the source `q` reads from, never null-extended?
fn is_level_source(q: &Query, alias: &str) -> bool {
    matches!(&q.from, Src::Table { alias: a, .. } if a == alias)
}

/// Is `c` false whenever the field it reads is null? `!=` and `is null`
/// are not; neither is anything not sent to the holder.
fn false_on_null(c: &BCond) -> bool {
    match c {
        BCond::Cmp { op, .. } => *op != CmpOp::Ne,
        BCond::In { .. } | BCond::PeerCaller { .. } => true,
        _ => false,
    }
}

/// Does `q` only page and project the sub-select it reads? Then the read's
/// rows are that sub-select's, and its window is the read's.
fn passes(q: &Query) -> bool {
    matches!(q.from, Src::Sub { .. })
        && q.joins.is_empty()
        && q.filters.is_empty()
        && q.having.is_empty()
        && q.group.is_none()
        && !q.distinct
        && q.source_joins.is_empty()
        && q.limit.is_none()
}

/// The level whose rows are the read's: `q`, or the sub-select a pass-
/// through wrapper reads, and whether a wrapper was crossed.
fn root_level(q: &Query) -> (&Query, bool) {
    let mut at = q;
    let mut wrapped = false;
    while passes(at) {
        let Src::Sub { query, .. } = &at.from else {
            break;
        };
        at = query;
        wrapped = true;
    }
    (at, wrapped)
}

fn root_level_mut(q: &mut Query) -> &mut Query {
    if !passes(q) {
        return q;
    }
    match &mut q.from {
        Src::Sub { query, .. } => root_level_mut(query),
        _ => unreachable!("a pass-through wrapper reads a sub-select"),
    }
}

/// Add `expr` (over an alias of the root level) to the read's outputs as
/// `name`, through each pass-through wrapper.
fn push_output(q: &mut Query, name: &str, expr: BExpr) {
    if passes(q) {
        if let Src::Sub { query, alias } = &mut q.from {
            let alias = alias.clone();
            push_output(query, name, expr);
            q.outputs.push(ordo_sema::ir::OutField {
                name: name.to_string(),
                expr: BExpr::Col {
                    alias,
                    column: name.to_string(),
                    ty: ScalarTy::Text,
                },
            });
            return;
        }
    }
    q.outputs.push(ordo_sema::ir::OutField {
        name: name.to_string(),
        expr,
    });
}

fn nested_in_cond(c: &BCond, externals: &ordo_sema::ir::ExternalReads, out: &mut Vec<Site>) {
    match c {
        BCond::Exists { query, .. } => collect(query, externals, false, out),
        BCond::And(items) | BCond::Or(items) => {
            items.iter().for_each(|i| nested_in_cond(i, externals, out))
        }
        BCond::Not(inner) => nested_in_cond(inner, externals, out),
        _ => {}
    }
}

/// The aliases an expression reads.
fn expr_aliases(e: &BExpr, out: &mut BTreeSet<String>) {
    match e {
        BExpr::Col { alias, .. } => {
            out.insert(alias.clone());
        }
        BExpr::Neg { expr, .. } => expr_aliases(expr, out),
        BExpr::Binary { lhs, rhs, .. } => {
            expr_aliases(lhs, out);
            expr_aliases(rhs, out);
        }
        BExpr::Fn { args, .. } => args.iter().for_each(|a| expr_aliases(a, out)),
        BExpr::Cond { arms, else_, .. } => {
            for (c, v) in arms {
                cond_aliases(c, out);
                expr_aliases(v, out);
            }
            expr_aliases(else_, out);
        }
        BExpr::Agg {
            arg, arg2, mask, ..
        } => {
            if let Some(a) = arg {
                expr_aliases(a, out);
            }
            if let Some(a) = arg2 {
                expr_aliases(a, out);
            }
            if let Some(m) = mask {
                cond_aliases(m, out);
            }
        }
        BExpr::RowNumber {
            partition, order, ..
        } => {
            partition.iter().for_each(|p| expr_aliases(p, out));
            order.iter().for_each(|s| expr_aliases(&s.expr, out));
        }
        BExpr::DocPath { doc, .. } => expr_aliases(doc, out),
        _ => {}
    }
}

/// The aliases a condition reads, a nested query's correlations included.
fn cond_aliases(c: &BCond, out: &mut BTreeSet<String>) {
    match c {
        BCond::Cmp { lhs, rhs, .. } | BCond::Text { lhs, rhs, .. } => {
            expr_aliases(lhs, out);
            expr_aliases(rhs, out);
        }
        BCond::In { expr, list } => {
            expr_aliases(expr, out);
            list.iter().for_each(|e| expr_aliases(e, out));
        }
        BCond::IsNull { expr, .. } | BCond::Bare(expr) => expr_aliases(expr, out),
        BCond::And(items) | BCond::Or(items) => items.iter().for_each(|i| cond_aliases(i, out)),
        BCond::Not(inner) => cond_aliases(inner, out),
        BCond::Exists { query, .. } => {
            let mut inner = BTreeSet::new();
            query_aliases(query, &mut inner);
            out.extend(inner);
        }
        BCond::PeerCaller { expr, caller, .. } => {
            expr_aliases(expr, out);
            expr_aliases(caller, out);
        }
        BCond::TreeWalk { expr, anchor, .. } => {
            expr_aliases(expr, out);
            expr_aliases(anchor, out);
        }
    }
}

fn query_aliases(q: &Query, out: &mut BTreeSet<String>) {
    q.filters
        .iter()
        .chain(&q.having)
        .for_each(|c| cond_aliases(c, out));
    q.outputs.iter().for_each(|o| expr_aliases(&o.expr, out));
    for j in &q.joins {
        if let Some(g) = &j.guard {
            cond_aliases(g, out);
        }
    }
}

/// Does `c` read `alias` and nothing else (no nested query)?
fn only_reads(c: &BCond, alias: &str) -> bool {
    if matches!(c, BCond::Exists { .. }) {
        return false;
    }
    let mut a = BTreeSet::new();
    cond_aliases(c, &mut a);
    // `$caller` is a parameter, not an alias, so a `PeerCaller` reads its
    // field's alias only.
    a.len() == 1 && a.contains(alias)
}

/// `field == $caller` on `alias`, as written by the binder: its column.
fn caller_on(c: &BCond, alias: &str) -> Option<String> {
    match c {
        BCond::PeerCaller {
            expr: BExpr::Col {
                alias: a, column, ..
            },
            ..
        } if a == alias => Some(column.clone()),
        _ => None,
    }
}

/// Refuse a `$caller` on a door alias in a place the holder cannot be sent
/// it: under `or` or `not`, or on a field that is not one of its columns.
fn check_callers(q: &Query, sites: &[Site]) -> Result<(), ExecFail> {
    fn walk(c: &BCond, sites: &[Site], plain: bool) -> Result<(), PeerReadFail> {
        match c {
            BCond::PeerCaller {
                expr: BExpr::Col { alias, column, .. },
                ..
            } => {
                let Some(s) = sites.iter().find(|s| &s.alias == alias) else {
                    return Ok(());
                };
                if !plain || !s.callers.contains(column) {
                    return Err(PeerReadFail::refused(format!(
                        "`{column} == $caller` on a peer read through its door (`peer {}`) \
                         is sent to the holder, which decides who `$caller` is there; it must \
                         be a plain condition of the read, not under `or` or `not`. Fix: move \
                         it out of the `or`, or read the peer's own guarded op",
                        s.read.door.clone().unwrap_or_default()
                    )));
                }
                Ok(())
            }
            BCond::PeerCaller { .. } => Ok(()),
            BCond::And(items) => items.iter().try_for_each(|i| walk(i, sites, plain)),
            BCond::Or(items) => items.iter().try_for_each(|i| walk(i, sites, false)),
            BCond::Not(inner) => walk(inner, sites, false),
            BCond::Exists { query, .. } => walk_query(query, sites),
            _ => Ok(()),
        }
    }
    fn walk_query(q: &Query, sites: &[Site]) -> Result<(), PeerReadFail> {
        for c in q.filters.iter().chain(&q.having) {
            walk(c, sites, true)?;
        }
        for j in &q.joins {
            if let Some(g) = &j.guard {
                walk(g, sites, true)?;
            }
        }
        for sj in &q.source_joins {
            if let Some(g) = &sj.guard {
                walk(g, sites, true)?;
            }
            if let Src::Sub { query, .. } = &sj.from {
                walk_query(query, sites)?;
            }
        }
        match &q.from {
            Src::Sub { query, .. } => walk_query(query, sites)?,
            Src::Union { arms, .. } => {
                for a in arms {
                    walk_query(a, sites)?;
                }
            }
            _ => {}
        }
        for c in &q.ctes {
            walk_query(&c.query, sites)?;
        }
        Ok(())
    }
    walk_query(q, sites).map_err(ExecFail::from)
}

/// Every `field == $caller` on a door alias becomes "the holder found
/// one": `field IS NOT NULL`. Each served row of that alias was answered
/// under the holder's `field == $caller`, and a row the join did not find
/// is null, as the peer path's `PeerCaller` was false for it.
fn rewrite_callers(q: &mut Query, sites: &[Site]) {
    let door: BTreeSet<&str> = sites.iter().map(|s| s.alias.as_str()).collect();
    fn cond(c: &mut BCond, door: &BTreeSet<&str>) {
        match c {
            BCond::PeerCaller {
                expr: e @ BExpr::Col { .. },
                ..
            } => {
                let BExpr::Col { alias, .. } = &*e else {
                    return;
                };
                if door.contains(alias.as_str()) {
                    *c = BCond::IsNull {
                        expr: e.clone(),
                        negated: true,
                    };
                }
            }
            BCond::And(items) | BCond::Or(items) => items.iter_mut().for_each(|i| cond(i, door)),
            BCond::Not(inner) => cond(inner, door),
            BCond::Exists { query, .. } => query_mut(query, door),
            _ => {}
        }
    }
    fn query_mut(q: &mut Query, door: &BTreeSet<&str>) {
        q.filters.iter_mut().for_each(|c| cond(c, door));
        q.having.iter_mut().for_each(|c| cond(c, door));
        for j in &mut q.joins {
            if let Some(g) = &mut j.guard {
                cond(g, door);
            }
        }
        for sj in &mut q.source_joins {
            if let Some(g) = &mut sj.guard {
                cond(g, door);
            }
            if let Src::Sub { query, .. } = &mut sj.from {
                query_mut(query, door);
            }
        }
        match &mut q.from {
            Src::Sub { query, .. } => query_mut(query, door),
            Src::Union { arms, .. } => arms.iter_mut().for_each(|a| query_mut(a, door)),
            _ => {}
        }
        for c in &mut q.ctes {
            query_mut(&mut c.query, door);
        }
    }
    query_mut(q, &door);
}

/// Drop the outermost filters that read one of `aliases`. True when one
/// was dropped.
fn drop_filters_on(q: &mut Query, aliases: &BTreeSet<String>) -> bool {
    let flat: Vec<BCond> = conjuncts(&q.filters).into_iter().cloned().collect();
    let before = flat.len();
    q.filters = flat
        .into_iter()
        .filter(|c| {
            let mut a = BTreeSet::new();
            cond_aliases(c, &mut a);
            a.is_disjoint(aliases)
        })
        .collect();
    q.filters.len() < before
}

/// Put `rows` into every copy of the program's external reads in `q`.
fn set_served(
    q: &mut Query,
    externals: &ordo_sema::ir::ExternalReads,
    rows: BTreeMap<String, Vec<Vec<Option<String>>>>,
) {
    let served: ServedRows = Arc::new(rows);
    let map: BTreeMap<String, ExternalRead> = externals
        .iter()
        .map(|(k, r)| {
            let mut r = r.clone();
            if r.door.is_some() {
                r.served = Some(served.clone());
            }
            (k.clone(), r)
        })
        .collect();
    let map = Arc::new(map);
    fn walk(q: &mut Query, map: &ordo_sema::ir::ExternalReads) {
        if q.externals.is_some() {
            q.externals = Some(map.clone());
        }
        match &mut q.from {
            Src::Sub { query, .. } => walk(query, map),
            Src::Union { arms, .. } => arms.iter_mut().for_each(|a| walk(a, map)),
            _ => {}
        }
        for c in &mut q.ctes {
            walk(&mut c.query, map);
        }
    }
    walk(q, &map);
}

/// The fetch every place starts from: the conditions the holder can be
/// sent, and the fields this read maps.
fn base_fetch(s: &Site) -> Fetch {
    let r = &s.read;
    let mut pick = vec![r.key_column.clone()];
    for c in &r.columns {
        if !pick.contains(&c.column) {
            pick.push(c.column.clone());
        }
    }
    let mut conds: Vec<Value> = s.own.iter().filter_map(|c| pushed(c, r)).collect();
    for field in &s.callers {
        if let Some(col) = holder_column(r, field) {
            let c = json!({ "field": col, "op": "caller" });
            if !conds.contains(&c) {
                conds.push(c);
            }
        }
    }
    Fetch {
        peer: r.door.clone().unwrap_or_default(),
        entity: r.table.clone(),
        conds,
        pick,
        sort: Vec::new(),
        limit: None,
        whole: true,
    }
}

/// The holder's field for this read's field `field`.
fn holder_column(r: &ExternalRead, field: &str) -> Option<String> {
    if field == "id" {
        return Some(r.key_column.clone());
    }
    r.columns
        .iter()
        .find(|c| c.field == field)
        .map(|c| c.column.clone())
}

/// One condition in the door's shape, when the holder can run it as
/// written: a comparison of one of this alias's fields with a literal.
fn pushed(c: &BCond, r: &ExternalRead) -> Option<Value> {
    let op_name = |op: CmpOp| match op {
        CmpOp::Eq => "eq",
        CmpOp::Ne => "ne",
        CmpOp::Lt => "lt",
        CmpOp::Le => "le",
        CmpOp::Gt => "gt",
        CmpOp::Ge => "ge",
    };
    let flip = |op: CmpOp| match op {
        CmpOp::Lt => CmpOp::Gt,
        CmpOp::Le => CmpOp::Ge,
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::Ge => CmpOp::Le,
        other => other,
    };
    match c {
        BCond::Cmp { op, lhs, rhs } => {
            let (col, lit, op) = match (lhs, rhs) {
                (BExpr::Col { column, .. }, BExpr::Lit { value, .. }) => (column, value, *op),
                (BExpr::Lit { value, .. }, BExpr::Col { column, .. }) => (column, value, flip(*op)),
                _ => return None,
            };
            let field = holder_column(r, col)?;
            let value = literal(r, col, lit)?;
            Some(json!({ "field": field, "op": op_name(op), "value": value }))
        }
        BCond::In {
            expr: BExpr::Col { column, .. },
            list,
        } => {
            let field = holder_column(r, column)?;
            let values = list
                .iter()
                .map(|e| match e {
                    BExpr::Lit { value, .. } => literal(r, column, value),
                    _ => None,
                })
                .collect::<Option<Vec<Value>>>()?;
            Some(json!({ "field": field, "op": "in", "value": values }))
        }
        BCond::IsNull {
            expr: BExpr::Col { column, .. },
            negated: false,
        } => {
            let field = holder_column(r, column)?;
            Some(json!({ "field": field, "op": "is_null" }))
        }
        _ => None,
    }
}

/// A literal as the holder's door takes it for `field`. Only the types
/// whose wire shape is their literal's are sent; a money, decimal or
/// duration comparison stays here.
fn literal(r: &ExternalRead, field: &str, lit: &Expr) -> Option<Value> {
    if field == "id" {
        let Expr::StrLit { value, .. } = lit else {
            return None;
        };
        return ordo_plan::external_key_of(value, r).map(|k| key_value(r, k));
    }
    let col = r.columns.iter().find(|c| c.field == field)?;
    if let Some(prefix) = &col.ref_prefix {
        // A ref to another external is shown as that entity's id.
        let Expr::StrLit { value, .. } = lit else {
            return None;
        };
        return value
            .strip_prefix(prefix.as_str())
            .and_then(|v| v.strip_prefix('_'))
            .map(|v| Value::String(v.to_string()));
    }
    match (col.ty, lit) {
        (ScalarTy::Text | ScalarTy::Id | ScalarTy::Uri, Expr::StrLit { value, .. }) => {
            Some(Value::String(value.clone()))
        }
        (ScalarTy::Int, Expr::IntLit { value, .. }) => i64::try_from(*value).ok().map(Value::from),
        (ScalarTy::Bool, Expr::BoolLit { value, .. }) => Some(Value::Bool(*value)),
        (ScalarTy::Date, Expr::DateLit { y, m, d, .. }) => {
            Some(Value::String(format!("{y:04}-{m:02}-{d:02}")))
        }
        (
            ScalarTy::Datetime,
            Expr::DatetimeLit {
                y,
                m,
                d,
                hh,
                mm,
                ss,
                ..
            },
        ) => Some(Value::String(format!(
            "{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"
        ))),
        _ => None,
    }
}

/// A raw key as the holder's door takes it.
fn key_value(r: &ExternalRead, raw: &str) -> Value {
    match r.key_ty {
        ordo_syntax::ExternalKeyTy::Int => raw
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(raw.to_string())),
        _ => Value::String(raw.to_string()),
    }
}

/// Is the outermost read's own window unknowable from one answer? True
/// when the read pages past its first page, or something narrows the
/// peer's rows here that the holder was not sent.
fn root_is_whole(q: &Query, sites: &[Site], paging: Option<Paging<'_>>) -> bool {
    let Some(root) = sites.iter().find(|s| s.place == Place::Root) else {
        return true;
    };
    if paging.is_some_and(|p| p.after.is_some()) {
        return true;
    }
    if q.group.is_some() || q.distinct || !q.having.is_empty() || !q.source_joins.is_empty() {
        return true;
    }
    // Every condition of the read must have gone to the holder.
    conjuncts(&q.filters).iter().any(|c| {
        let sent = caller_on(c, &root.alias).is_some()
            || (only_reads(c, &root.alias) && pushed(c, &root.read).is_some());
        !sent
    })
}

/// The order and the number of rows to send for the outermost read: its
/// declared sort, then the id as here, and its `take` or the page.
fn root_window(
    q: &Query,
    s: &Site,
    paging: Option<Paging<'_>>,
) -> Option<(Vec<(String, bool)>, usize)> {
    let mut sort = Vec::new();
    for k in &q.sort {
        let BExpr::Col { alias, column, .. } = &k.expr else {
            return None;
        };
        if alias != &s.alias {
            return None;
        }
        let col = holder_column(&s.read, column)?;
        let ty = if column == "id" {
            None
        } else {
            s.read
                .columns
                .iter()
                .find(|c| &c.field == column)
                .map(|c| c.ty)
        };
        if matches!(ty, Some(ScalarTy::Json | ScalarTy::Blob)) {
            return None;
        }
        sort.push((col, k.desc));
    }
    let key = s.read.key_column.clone();
    if !sort.iter().any(|(f, _)| f == &key) {
        sort.push((key, false));
    }
    let take = match &q.limit {
        Some(ordo_sema::ir::Limit::Lit(n)) => Some((*n).max(0) as usize),
        _ => None,
    };
    let page = paging.map(|p| p.page_size.max(1) as usize);
    let n = match (take, page) {
        (Some(t), Some(p)) => t.min(p),
        (Some(t), None) => t,
        (None, Some(p)) => p,
        (None, None) => return None,
    };
    Some((sort, n.max(1)))
}

/// The holder's rows, in the order the planner reads them: the key, then
/// each declared column. A field the holder did not answer is refused by
/// name: the external maps a field the holder's list does not have.
fn to_rows(
    r: &ExternalRead,
    rows: &[Map<String, Value>],
) -> Result<Vec<Vec<Option<String>>>, ExecFail> {
    let peer = r.door.clone().unwrap_or_default();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut vals = Vec::with_capacity(r.columns.len() + 1);
        let get = |col: &str| {
            row.get(col).ok_or_else(|| {
                ExecFail::from(PeerReadFail::refused(format!(
                    "peer `{peer}` answered `std.{}.list` without the field `{col}`, which this \
                     deployment's external maps. Fix: map a field the holder's list has \
                     (`ordo introspect` on the holder), or add it to the holder's entity",
                    r.table
                )))
            })
        };
        vals.push(text_of(get(&r.key_column)?, None));
        for c in &r.columns {
            vals.push(text_of(get(&c.column)?, Some(c.ty)));
        }
        out.push(vals);
    }
    Ok(out)
}

/// A value of the door's answer as the text its column is cast from. A
/// money value comes in major units and is stored in minor ones.
fn text_of(v: &Value, ty: Option<ScalarTy>) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(match ty {
            Some(ScalarTy::Money(places)) => minor_units(s, places).unwrap_or_else(|| s.clone()),
            _ => s.clone(),
        }),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(match ty {
            Some(ScalarTy::Money(places)) => {
                minor_units(&n.to_string(), places).unwrap_or_else(|| n.to_string())
            }
            _ => n.to_string(),
        }),
        other => Some(other.to_string()),
    }
}

/// `"12.34"` at 2 places is `"1234"`.
fn minor_units(major: &str, places: u8) -> Option<String> {
    let (neg, body) = match major.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, major),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if frac.len() > places as usize
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let mut digits = format!("{int}{frac:0<width$}", width = places as usize);
    while digits.len() > 1 && digits.starts_with('0') {
        digits.remove(0);
    }
    Some(if neg { format!("-{digits}") } else { digits })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_comes_in_major_units_and_is_read_in_minor_ones() {
        assert_eq!(minor_units("12.34", 2).as_deref(), Some("1234"));
        assert_eq!(minor_units("12.3", 2).as_deref(), Some("1230"));
        assert_eq!(minor_units("-0.05", 2).as_deref(), Some("-5"));
        assert_eq!(minor_units("7", 0).as_deref(), Some("7"));
        assert_eq!(minor_units("1.234", 2), None);
    }

    #[test]
    fn the_refinement_carries_sort_and_limit_only_when_set() {
        let f = Fetch {
            peer: "notes".into(),
            entity: "notes".into(),
            conds: vec![json!({ "field": "owner", "op": "caller" })],
            pick: vec!["id".into(), "title".into()],
            sort: vec![("created".into(), true), ("id".into(), false)],
            limit: Some(10),
            whole: false,
        };
        let r = f.refine();
        assert_eq!(r["limit"], 10);
        assert_eq!(r["sort"][0], json!({ "field": "created", "desc": true }));
        assert_eq!(r["where"][0]["op"], "caller");
        let plain = Fetch {
            sort: Vec::new(),
            limit: None,
            ..f
        };
        assert!(plain.refine().get("sort").is_none());
        assert!(plain.refine().get("limit").is_none());
    }
}
