//! **Rollout strategies** (`plan-versions.md` §3.5, §3.7, increment 10c).
//!
//! [`crate::rollouts`] holds the rollout and its worker. This module holds
//! what the strategies beyond `rolling` and `all` need, each behind the
//! same client and the same refusal:
//!
//! - **Health** (§3.5): a tenant that moved is healthy while its server
//!   faults stay under `rollout.health_error_rate` of its calls. Every node
//!   adds what it counted ([`report`]) to `$rollout_health`, so the leader
//!   reads the whole deployment, not its own node. A fault is a 5xx the
//!   executor answered: a refusal of the caller's input is not the
//!   version's fault.
//! - **By query** (§3.5, V-46): an Ordo condition over a tenant-facts view
//!   Ordo computes from its own registry. Tenants cannot edit a fact, and
//!   the condition is evaluated here, never run as SQL, so it reads and
//!   writes nothing.
//! - **Share partners** (§1.7): two tenants are partners when one holds an
//!   open capability grant to a person who is a member of the other.
//! - **The alert channel** (§3.5 "told"): the deployment's stream always,
//!   and a mail to one address when `rollout.alert_channel` is
//!   `mail:<address>`.

use std::collections::{BTreeMap, BTreeSet};

use ordo_exec::route::PoolResolver;
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_syntax::ast::{CmpOp, Condition, Expr, TextOp};

// ── health ───────────────────────────────────────────────────────────────

/// `rollout.health_error_rate`'s default, in hundredths of a percent: 1%.
pub const HEALTH_ERROR_RATE_DEFAULT: i64 = 100;

/// `rollout.health_min_calls`' default: 20 calls (ruled 2026-11-25). A
/// first stage that had fewer when its wait ends has proved nothing, and
/// the rollout pauses.
pub const HEALTH_MIN_CALLS_DEFAULT: i64 = 20;

/// The largest `rollout.health_min_calls` takes.
pub const MAX_MIN_CALLS: i64 = 1_000_000_000;

/// Whether a first stage had too few calls to judge: fewer than `min`.
pub fn too_quiet(calls: u64, min: i64) -> bool {
    (calls as i128) < (min.max(0) as i128)
}

/// A rate as typed (`1%`, `0.5%`, `2`) in hundredths of a percent, from
/// 0.01% to 100%.
pub fn parse_rate(text: &str) -> Option<i64> {
    let t = text.trim().trim_end_matches('%').trim();
    if t.is_empty() || t.starts_with('-') {
        return None;
    }
    let (whole, frac) = t.split_once('.').unwrap_or((t, ""));
    if whole.is_empty() && frac.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || frac.len() > 2
    {
        return None;
    }
    let w: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let f: i64 = format!("{frac:0<2}").parse().ok()?;
    let n = w.checked_mul(100)?.checked_add(f)?;
    (1..=10_000).contains(&n).then_some(n)
}

/// A rate in hundredths of a percent, as a person reads it: `1%`, `0.5%`.
pub fn format_rate(h: i64) -> String {
    let (w, f) = (h / 100, h % 100);
    match f {
        0 => format!("{w}%"),
        f if f % 10 == 0 => format!("{w}.{}%", f / 10),
        f => format!("{w}.{f:02}%"),
    }
}

/// Whether `faults` of `calls` reach the rate: unhealthy. No calls is no
/// fault; too few calls is judged apart ([`too_quiet`]).
pub fn unhealthy(calls: u64, faults: u64, rate: i64) -> bool {
    faults > 0 && (faults as u128) * 10_000 >= (calls as u128) * (rate.max(1) as u128)
}

const HEALTH_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$rollout_health\" ( \
  \"rollout\" bigint NOT NULL, \
  \"space\" text NOT NULL, \
  \"node\" text NOT NULL, \
  \"calls\" bigint NOT NULL DEFAULT 0, \
  \"faults\" bigint NOT NULL DEFAULT 0, \
  \"last_calls\" bigint NOT NULL DEFAULT 0, \
  \"last_faults\" bigint NOT NULL DEFAULT 0, \
  \"at\" bigint NOT NULL, \
  PRIMARY KEY (\"rollout\", \"space\", \"node\") \
)";

/// The health table. Called from `Rollouts::ensure`.
pub(crate) fn health_ddl() -> &'static str {
    HEALTH_DDL
}

/// **Add what this node counted** for the tenants the open rollout has
/// moved while it waits on its first stage's health. `totals` is this
/// node's calls and faults per tenant since it started
/// (`ObservedCosts::per_space_totals`). The first sight of a tenant is its
/// baseline; after that each report adds the difference. A count lower
/// than the last one is a restarted node, which counts again from zero.
pub async fn report(
    pool: &Pool,
    node: &str,
    totals: &BTreeMap<String, (u64, u64)>,
    now: u64,
) -> Result<usize, PoolError> {
    let open = match pool
        .rows(
            "SELECT \"id\" FROM \"$rollouts\" WHERE \"state\" = 'running' \
             AND \"gate\" = 'pending' LIMIT 1",
        )
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_missing_table() || e.to_string().contains("gate") => return Ok(0),
        Err(e) => return Err(e),
    };
    let Some(id) = open
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|v| v.parse::<i64>().ok())
    else {
        return Ok(0);
    };
    let moved = pool
        .rows(&format!(
            "SELECT t.\"space\", h.\"last_calls\", h.\"last_faults\" FROM \"$rollout_tenants\" t \
             LEFT JOIN \"$rollout_health\" h ON h.\"rollout\" = t.\"rollout\" \
               AND h.\"space\" = t.\"space\" AND h.\"node\" = {node} \
             WHERE t.\"rollout\" = {id} AND t.\"state\" = 'moved' \
               AND t.\"space\" <> {step}",
            node = sql_lit(node),
            step = sql_lit(crate::rollouts::DEPLOYMENT_STEP),
        ))
        .await?;
    let mut n = 0;
    for row in moved {
        let Some(space) = row.first().cloned().flatten() else {
            continue;
        };
        let (calls, faults) = totals.get(&space).copied().unwrap_or((0, 0));
        let last = |i: usize| {
            row.get(i)
                .cloned()
                .flatten()
                .and_then(|v| v.parse::<u64>().ok())
        };
        let sql = match (last(1), last(2)) {
            (Some(lc), Some(lf)) => {
                let dc = if calls >= lc { calls - lc } else { calls };
                let df = if faults >= lf { faults - lf } else { faults };
                if dc == 0 && df == 0 && calls == lc && faults == lf {
                    continue;
                }
                format!(
                    "UPDATE \"$rollout_health\" SET \"calls\" = \"calls\" + {dc}, \
                       \"faults\" = \"faults\" + {df}, \"last_calls\" = {calls}, \
                       \"last_faults\" = {faults}, \"at\" = {now} \
                     WHERE \"rollout\" = {id} AND \"space\" = {s} AND \"node\" = {node}",
                    s = sql_lit(&space),
                    node = sql_lit(node),
                )
            }
            _ => format!(
                "INSERT INTO \"$rollout_health\" (\"rollout\", \"space\", \"node\", \
                   \"last_calls\", \"last_faults\", \"at\") \
                 VALUES ({id}, {s}, {node}, {calls}, {faults}, {now}) \
                 ON CONFLICT DO NOTHING",
                s = sql_lit(&space),
                node = sql_lit(node),
            ),
        };
        pool.rows(&sql).await?;
        n += 1;
    }
    Ok(n)
}

/// The calls and faults every node counted for rollout `id`'s moved
/// tenants.
pub async fn health_of(pool: &Pool, id: i64) -> Result<(u64, u64), PoolError> {
    let rows = match pool
        .rows(&format!(
            "SELECT coalesce(sum(\"calls\"), 0), coalesce(sum(\"faults\"), 0) \
             FROM \"$rollout_health\" WHERE \"rollout\" = {id}"
        ))
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_missing_table() => return Ok((0, 0)),
        Err(e) => return Err(e),
    };
    let n = |i: usize| {
        rows.first()
            .and_then(|r| r.get(i).cloned().flatten())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    Ok((n(0), n(1)))
}

// ── the alert channel ────────────────────────────────────────────────────

/// `rollout.alert_channel`'s default: the deployment's stream.
pub const ALERT_STREAM: &str = "stream";

/// The outbox kind a rollout's alert is queued under.
pub const ALERT_KIND: &str = "rollout.alert";

/// A channel as typed, checked: `stream`, or `mail:<address>`.
pub fn check_channel(text: &str) -> Result<String, String> {
    let t = text.trim();
    if t == ALERT_STREAM {
        return Ok(t.to_string());
    }
    match t.strip_prefix("mail:") {
        Some(addr) => crate::mail::MailAddress::parse(addr.trim())
            .map(|a| format!("mail:{}", a.as_str()))
            .map_err(|e| format!("`{}` is not a mail address ({e})", addr.trim())),
        None => Err(
            "it is `stream` (the deployment's stream) or `mail:<address>` (the stream and \
             a mail)"
                .into(),
        ),
    }
}

/// **Tell the deployment owner** on the rollout's channel. The stream entry
/// is the caller's (`ApplyOps::record`); this queues the mail when the
/// channel has one. `Ok(true)` when a mail was queued.
pub async fn tell(
    pool: &Pool,
    channel: &str,
    subject: &str,
    body: &str,
    now: u64,
) -> Result<bool, String> {
    let Some(addr) = channel.strip_prefix("mail:") else {
        return Ok(false);
    };
    let to = crate::mail::MailAddress::parse(addr).map_err(|e| e.to_string())?;
    let payload: String = format!("{subject}\n{body}").chars().take(4000).collect();
    let job = crate::mail::MailJob::new(ALERT_KIND, to, &payload).map_err(|e| e.to_string())?;
    crate::mail::enqueue(pool, &job, now)
        .await
        .map_err(|e| format!("the alert mail was not queued: {e}"))?;
    Ok(true)
}

/// Writes a rollout's alert at send time: the payload's first line is the
/// subject, the rest the text.
pub struct RolloutAlert;

impl crate::mail::Composer for RolloutAlert {
    fn compose(
        &self,
        job: &crate::mail::QueuedMail,
        _now: u64,
    ) -> Result<Option<crate::mail::Message>, crate::mail::MailError> {
        let (subject, body) = job
            .payload
            .split_once('\n')
            .unwrap_or((job.payload.as_str(), ""));
        crate::mail::Message::new(
            job.to.clone(),
            subject.trim(),
            &format!(
                "{}\n\n`ordo rollout status` shows where it stands.\n",
                body.trim()
            ),
        )
        .map(Some)
    }
}

// ── the tenant facts and the query ──────────────────────────────────────

/// A fact's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactTy {
    Text,
    Int,
    Bool,
}

/// **The tenant-facts view**: what a query may read, each from Ordo's own
/// records. A tenant edits none of them. `rows.<entity>` is one more, per
/// entity: [`ROWS_PREFIX`].
pub const FACTS: [(&str, FactTy, &str); 11] = [
    ("tenant", FactTy::Text, "the tenant's id"),
    ("name", FactTy::Text, "the tenant's name"),
    ("version", FactTy::Text, "the version it is on"),
    (
        "createdAt",
        FactTy::Int,
        "when it was made, in seconds since 1970",
    ),
    ("ageDays", FactTy::Int, "whole days since it was made"),
    ("members", FactTy::Int, "its members"),
    (
        "requests30d",
        FactTy::Int,
        "requests served in the last 30 days, from the usage ledger",
    ),
    (
        "writes30d",
        FactTy::Int,
        "writes in the last 30 days, from the usage rollup",
    ),
    ("personal", FactTy::Bool, "a person's own tenant"),
    ("ownDatabase", FactTy::Bool, "it has a database of its own"),
    ("paused", FactTy::Bool, "it is paused"),
];

/// `rows.<entity>`: how many rows of that entity the tenant holds.
pub const ROWS_PREFIX: &str = "rows";

/// The window the usage facts cover.
pub const USAGE_WINDOW_SECS: u64 = 30 * 86_400;

fn fact_ty(name: &str) -> Option<(&'static str, FactTy)> {
    FACTS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(n, t, _)| (*n, *t))
}

fn fact_names() -> String {
    let mut names: Vec<&str> = FACTS.iter().map(|(n, _, _)| *n).collect();
    names.push("rows.<entity>");
    names.join(", ")
}

/// An entity name as a table name may carry it.
fn entity_name(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && text.len() <= 63
}

/// A fact named in a query: one of [`FACTS`], or `rows.<entity>`.
fn named_fact(segments: &[&str]) -> Result<(String, FactTy), String> {
    match segments {
        [one] => fact_ty(one)
            .map(|(n, t)| (n.to_string(), t))
            .ok_or_else(|| format!("`{one}` is not a tenant fact; there are {}", fact_names())),
        [ROWS_PREFIX, entity] if entity_name(entity) => {
            Ok((format!("{ROWS_PREFIX}.{entity}"), FactTy::Int))
        }
        _ => Err(format!(
            "`{}` is not a tenant fact; there are {}",
            segments.join("."),
            fact_names()
        )),
    }
}

/// One fact's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Text(String),
    Int(i64),
    Bool(bool),
    Null,
}

/// One tenant's facts.
#[derive(Debug, Clone)]
pub struct Facts {
    pub tenant: String,
    values: BTreeMap<String, Fact>,
}

impl Facts {
    pub fn get(&self, name: &str) -> &Fact {
        self.values.get(name).unwrap_or(&Fact::Null)
    }
}

/// What a query reads beyond the registry, so nothing else is counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Needs {
    pub members: bool,
    pub usage: bool,
    /// The entities `rows.<entity>` names.
    pub rows: BTreeSet<String>,
}

impl Needs {
    fn note(&mut self, fact: &str) {
        match fact {
            "members" => self.members = true,
            "requests30d" | "writes30d" => self.usage = true,
            f => {
                if let Some(e) = f.strip_prefix("rows.") {
                    self.rows.insert(e.to_string());
                }
            }
        }
    }
}

/// A query as checked: which tenants, in which order, how many.
#[derive(Debug, Clone)]
pub struct Query {
    cond: Condition,
    order: Vec<(String, bool)>,
    limit: Option<usize>,
    needs: Needs,
}

/// One side of a comparison, checked.
enum Side {
    Fact(String, FactTy),
    Lit(Fact),
}

impl Side {
    fn ty(&self) -> Option<FactTy> {
        match self {
            Side::Fact(_, t) => Some(*t),
            Side::Lit(Fact::Text(_)) => Some(FactTy::Text),
            Side::Lit(Fact::Int(_)) => Some(FactTy::Int),
            Side::Lit(Fact::Bool(_)) => Some(FactTy::Bool),
            Side::Lit(Fact::Null) => None,
        }
    }

    fn value<'a>(&'a self, f: &'a Facts) -> &'a Fact {
        match self {
            Side::Fact(n, _) => f.get(n),
            Side::Lit(v) => v,
        }
    }
}

fn side(e: &Expr) -> Result<Side, String> {
    match e {
        Expr::Path { segments, .. } => {
            let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
            named_fact(&names).map(|(n, t)| Side::Fact(n, t))
        }
        Expr::IntLit { value, .. } => i64::try_from(*value)
            .map(|v| Side::Lit(Fact::Int(v)))
            .map_err(|_| "a number past 64 bits".into()),
        Expr::Neg { expr, .. } => match expr.as_ref() {
            Expr::IntLit { value, .. } => i64::try_from(-*value)
                .map(|v| Side::Lit(Fact::Int(v)))
                .map_err(|_| "a number past 64 bits".into()),
            _ => Err("a query compares facts with values; it does no arithmetic".into()),
        },
        Expr::StrLit { value, .. } => Ok(Side::Lit(Fact::Text(value.clone()))),
        Expr::BoolLit { value, .. } => Ok(Side::Lit(Fact::Bool(*value))),
        Expr::NullLit { .. } => Ok(Side::Lit(Fact::Null)),
        _ => Err(
            "a query compares a tenant fact with a number, text, true or false; it does no \
             arithmetic, calls no function and takes no parameter"
                .into(),
        ),
    }
}

fn check(c: &Condition, needs: &mut Needs) -> Result<(), String> {
    let note = |s: &Side, needs: &mut Needs| {
        if let Side::Fact(n, _) = s {
            needs.note(n);
        }
    };
    match c {
        Condition::Compare { op, lhs, rhs, .. } => {
            let (l, r) = (side(lhs)?, side(rhs)?);
            note(&l, needs);
            note(&r, needs);
            match (l.ty(), r.ty()) {
                (Some(a), Some(b)) if a == b => {
                    if a == FactTy::Bool && !matches!(op, CmpOp::Eq | CmpOp::Ne) {
                        return Err("true and false compare with == and != only".into());
                    }
                    Ok(())
                }
                (None, _) | (_, None) => {
                    Err("compare with null by `is null` or `is not null`".into())
                }
                _ => Err(format!(
                    "`{}` compares a {:?} with a {:?}",
                    op.as_str(),
                    l.ty().unwrap_or(FactTy::Text),
                    r.ty().unwrap_or(FactTy::Text)
                )),
            }
        }
        Condition::In { expr, list, .. } => {
            let s = side(expr)?;
            note(&s, needs);
            for e in list {
                let v = side(e)?;
                if v.ty() != s.ty() || matches!(v, Side::Fact(..)) {
                    return Err("`in` lists values of the fact's own type".into());
                }
            }
            Ok(())
        }
        Condition::Text { op, lhs, rhs, .. } => {
            if *op == TextOp::Matches {
                return Err(
                    "`matches` is not offered over tenant facts; use `contains` or `starts`".into(),
                );
            }
            let (l, r) = (side(lhs)?, side(rhs)?);
            if l.ty() != Some(FactTy::Text) || r.ty() != Some(FactTy::Text) {
                return Err(format!("`{}` is for text", op.as_str()));
            }
            Ok(())
        }
        Condition::IsNull { expr, .. } => {
            let s = side(expr)?;
            note(&s, needs);
            Ok(())
        }
        Condition::BareBool(e) => match side(e)? {
            Side::Fact(_, FactTy::Bool) | Side::Lit(Fact::Bool(_)) => Ok(()),
            _ => Err("a condition on its own is a true or false fact, like `personal`".into()),
        },
        Condition::And { items, .. } | Condition::Or { items, .. } => {
            items.iter().try_for_each(|i| check(i, needs))
        }
        Condition::Not { inner, .. } => check(inner, needs),
        Condition::Since { .. } | Condition::Under { .. } | Condition::Above { .. } => Err(
            "`since`, `under` and `above` are not offered over tenant facts; compare \
             `ageDays` or `createdAt`"
                .into(),
        ),
    }
}

fn eval(c: &Condition, f: &Facts) -> bool {
    match c {
        Condition::Compare { op, lhs, rhs, .. } => {
            let (Ok(l), Ok(r)) = (side(lhs), side(rhs)) else {
                return false;
            };
            let ord = match (l.value(f), r.value(f)) {
                (Fact::Int(x), Fact::Int(y)) => x.cmp(y),
                (Fact::Text(x), Fact::Text(y)) => x.cmp(y),
                (Fact::Bool(x), Fact::Bool(y)) => x.cmp(y),
                _ => return false,
            };
            match op {
                CmpOp::Eq => ord.is_eq(),
                CmpOp::Ne => ord.is_ne(),
                CmpOp::Lt => ord.is_lt(),
                CmpOp::Le => ord.is_le(),
                CmpOp::Gt => ord.is_gt(),
                CmpOp::Ge => ord.is_ge(),
            }
        }
        Condition::In { expr, list, .. } => {
            let Ok(s) = side(expr) else { return false };
            let v = s.value(f);
            *v != Fact::Null
                && list
                    .iter()
                    .filter_map(|e| side(e).ok())
                    .any(|e| e.value(f) == v)
        }
        Condition::Text { op, lhs, rhs, .. } => {
            let (Ok(l), Ok(r)) = (side(lhs), side(rhs)) else {
                return false;
            };
            match (l.value(f), r.value(f), op) {
                (Fact::Text(a), Fact::Text(b), TextOp::Contains) => a.contains(b.as_str()),
                (Fact::Text(a), Fact::Text(b), TextOp::Starts) => a.starts_with(b.as_str()),
                _ => false,
            }
        }
        Condition::IsNull { expr, negated, .. } => {
            let Ok(s) = side(expr) else { return false };
            (*s.value(f) == Fact::Null) != *negated
        }
        Condition::BareBool(e) => {
            matches!(side(e).map(|s| s.value(f).clone()), Ok(Fact::Bool(true)))
        }
        Condition::And { items, .. } => items.iter().all(|i| eval(i, f)),
        Condition::Or { items, .. } => items.iter().any(|i| eval(i, f)),
        Condition::Not { inner, .. } => !eval(inner, f),
        _ => false,
    }
}

impl Query {
    /// **Check a query**: an Ordo condition over [`FACTS`] and
    /// `rows.<entity>`, an order (`<fact> [asc|desc]`, comma separated),
    /// and how many of them at most (`rollout.limit`, "the 5 least used").
    /// Refused naming what is wrong and the facts there are.
    pub fn parse(select: &str, order: Option<&str>) -> Result<Query, String> {
        let cond = ordo_syntax::parser::parse_condition(select.trim())
            .map_err(|e| format!("it does not parse: {}", e.message))?;
        let mut needs = Needs::default();
        check(&cond, &mut needs)?;
        let mut by = Vec::new();
        for part in order
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            let mut words = part.split_whitespace();
            let name = words.next().unwrap_or_default();
            let (name, _) = named_fact(&name.split('.').collect::<Vec<_>>())
                .map_err(|why| format!("the order names a fact there is not: {why}"))?;
            let desc = match words.next() {
                None | Some("asc") => false,
                Some("desc") => true,
                Some(w) => return Err(format!("the order says `{w}`; it is `asc` or `desc`")),
            };
            if words.next().is_some() {
                return Err("the order is `<fact> [asc|desc]`, comma separated".into());
            }
            needs.note(&name);
            by.push((name, desc));
        }
        Ok(Query {
            cond,
            order: by,
            limit: None,
            needs,
        })
    }

    /// At most `n` of the tenants it selects, the first in its order.
    pub fn limited(mut self, n: Option<usize>) -> Query {
        self.limit = n;
        self
    }

    pub fn needs(&self) -> &Needs {
        &self.needs
    }

    pub fn uses_members(&self) -> bool {
        self.needs.members
    }

    pub fn selects(&self, f: &Facts) -> bool {
        eval(&self.cond, f)
    }

    /// The tenants it selects, in its order, then by id, cut at its limit.
    /// A null sorts last either way.
    pub fn select(&self, facts: Vec<Facts>) -> Vec<String> {
        let mut chosen: Vec<Facts> = facts.into_iter().filter(|f| self.selects(f)).collect();
        chosen.sort_by(|a, b| {
            for (name, desc) in &self.order {
                let o = match (a.get(name), b.get(name)) {
                    (Fact::Null, Fact::Null) => std::cmp::Ordering::Equal,
                    (Fact::Null, _) => return std::cmp::Ordering::Greater,
                    (_, Fact::Null) => return std::cmp::Ordering::Less,
                    (Fact::Int(x), Fact::Int(y)) => x.cmp(y),
                    (Fact::Text(x), Fact::Text(y)) => x.cmp(y),
                    (Fact::Bool(x), Fact::Bool(y)) => x.cmp(y),
                    _ => std::cmp::Ordering::Equal,
                };
                let o = if *desc { o.reverse() } else { o };
                if o.is_ne() {
                    return o;
                }
            }
            a.tenant.cmp(&b.tenant)
        });
        if let Some(n) = self.limit {
            chosen.truncate(n);
        }
        chosen.into_iter().map(|f| f.tenant).collect()
    }
}

/// A count per tenant, or `None` when the table is not there.
async fn counts_by_space(
    pool: &Pool,
    sql: &str,
) -> Result<Option<BTreeMap<String, i64>>, PoolError> {
    match pool.rows(sql).await {
        Ok(r) => Ok(Some(
            r.into_iter()
                .filter_map(|r| {
                    Some((r.first()?.clone()?, r.get(1)?.clone()?.parse::<i64>().ok()?))
                })
                .collect(),
        )),
        Err(e) if e.is_missing_table() => Ok(None),
        Err(e) => Err(e),
    }
}

/// One tenant's count in its own database. `None` when it cannot be
/// reached or the table is not there: a fact unknown, never a zero.
async fn own_count(pools: &dyn PoolResolver, tenant: &str, sql: &str) -> Option<i64> {
    let pool = pools
        .resolve(&ordo_crypto::SpaceId(tenant.to_string()))
        .await
        .ok()?;
    pool.rows(sql)
        .await
        .ok()?
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|v| v.parse().ok())
}

fn members_sql(filter: &str) -> String {
    format!(
        "SELECT \"$space\", count(*) FROM \"members\" WHERE \"kind\" = 'principal'{filter} \
         GROUP BY 1"
    )
}

fn rows_sql(entity: &str, filter: &str) -> String {
    format!("SELECT \"$space\", count(*) FROM \"{entity}\" WHERE true{filter} GROUP BY 1")
}

/// **The facts of every serving tenant.** The registry is read from the
/// control database. What `needs` names beyond it is counted only then:
/// members and `rows.<entity>` on the deployment's database, and in a
/// tenant's own database through `pools` (unreachable reads as no value);
/// usage from the usage tables, none when the ledger was never on.
pub async fn facts(
    pool: &Pool,
    pools: Option<&dyn PoolResolver>,
    needs: &Needs,
    now: u64,
) -> Result<Vec<Facts>, PoolError> {
    let rows = pool
        .rows(
            "SELECT \"$space\", \"name\", \"version\", \"created_at\", \"own_of\" IS NOT NULL, \
               \"state\" FROM \"$spaces\" \
             WHERE \"state\" IN ('provisioning', 'active', 'paused') ORDER BY \"$space\"",
        )
        .await?;
    let own: BTreeSet<String> = match ordo_exec::spacedb::all(pool).await {
        Ok(a) => a.into_iter().map(|(s, _)| s.0).collect(),
        Err(e) if e.is_missing_table() => BTreeSet::new(),
        Err(e) => return Err(e),
    };
    // Counts on the deployment's database, by tenant.
    let mut shared: BTreeMap<String, Option<BTreeMap<String, i64>>> = BTreeMap::new();
    if needs.members {
        shared.insert(
            "members".into(),
            counts_by_space(pool, &members_sql("")).await?,
        );
    }
    for e in &needs.rows {
        shared.insert(
            format!("{ROWS_PREFIX}.{e}"),
            counts_by_space(pool, &rows_sql(e, "")).await?,
        );
    }
    if needs.usage {
        let since = now.saturating_sub(USAGE_WINDOW_SECS);
        shared.insert(
            "requests30d".into(),
            counts_by_space(
                pool,
                &format!(
                    "SELECT \"space\", coalesce(sum((SELECT coalesce(sum(j.v::bigint), 0) \
                       FROM jsonb_each_text(\"requests\") AS j(k, v))), 0)::bigint \
                     FROM \"$usage_replica\" WHERE \"period_start\" >= {since} GROUP BY 1"
                ),
            )
            .await?,
        );
        shared.insert(
            "writes30d".into(),
            counts_by_space(
                pool,
                &format!(
                    "SELECT \"space\", coalesce(sum(\"writes\"), 0)::bigint FROM \"$usage\" \
                     WHERE \"period_start\" >= {since} GROUP BY 1"
                ),
            )
            .await?,
        );
    }
    let mut out = Vec::new();
    for r in rows {
        let s = |i: usize| r.get(i).cloned().flatten();
        let Some(tenant) = s(0) else { continue };
        let created = s(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        let own_db = own.contains(&tenant);
        let mut values = BTreeMap::new();
        values.insert("tenant".to_string(), Fact::Text(tenant.clone()));
        values.insert("name".into(), Fact::Text(s(1).unwrap_or_default()));
        values.insert("version".into(), s(2).map(Fact::Text).unwrap_or(Fact::Null));
        values.insert("createdAt".into(), Fact::Int(created));
        values.insert(
            "ageDays".into(),
            Fact::Int((now as i64).saturating_sub(created).max(0) / 86_400),
        );
        values.insert(
            "personal".into(),
            Fact::Bool(matches!(s(4).as_deref(), Some("t" | "true"))),
        );
        values.insert("ownDatabase".into(), Fact::Bool(own_db));
        values.insert(
            "paused".into(),
            Fact::Bool(s(5).as_deref() == Some("paused")),
        );
        for (name, counts) in &shared {
            let usage = name.ends_with("30d");
            let v = if own_db && !usage {
                // Counted in the tenant's own database.
                let filter = format!(" AND \"$space\" = {}", sql_lit(&tenant));
                let sql = match name.strip_prefix("rows.") {
                    Some(e) => rows_sql(e, &filter),
                    None => members_sql(&filter),
                };
                let sql = format!("SELECT coalesce(sum(n), 0) FROM ({}) c(s, n)", sql);
                match pools {
                    Some(p) => own_count(p, &tenant, &sql).await.map(Fact::Int),
                    None => None,
                }
            } else {
                counts
                    .as_ref()
                    .map(|m| Fact::Int(m.get(&tenant).copied().unwrap_or(0)))
            };
            values.insert(name.clone(), v.unwrap_or(Fact::Null));
        }
        out.push(Facts { tenant, values });
    }
    Ok(out)
}

// ── share partners ──────────────────────────────────────────────────────

const GRANTS_SQL: &str = "SELECT \"$space\", \"principal\" FROM \"$capabilitygrant\" \
                          WHERE \"revoked_at\" IS NULL AND \"expires\" > ";

/// `(tenant, principal)` pairs from one database; nothing when the table
/// is not there or the database cannot be read.
async fn pairs(pool: &Pool, sql: &str) -> Vec<(String, String)> {
    pool.rows(sql)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| Some((r.first()?.clone()?, r.get(1)?.clone()?)))
        .collect()
}

/// **Every tenant's share partners**, both ways: a tenant that holds an
/// open capability grant to a person, and each tenant that person is a
/// member of. Grants and members are read on the deployment's database and
/// in each tenant's own database through `pools`; one that cannot be read
/// adds nothing.
pub async fn partners(
    pool: &Pool,
    pools: Option<&dyn PoolResolver>,
    now: u64,
) -> Result<BTreeMap<String, BTreeSet<String>>, PoolError> {
    let members_of = "SELECT \"$space\", \"principal\" FROM \"members\" \
                      WHERE \"kind\" = 'principal'";
    let mut grants = pairs(pool, &format!("{GRANTS_SQL}{now}")).await;
    let mut members = pairs(pool, members_of).await;
    if let Some(pools) = pools {
        let own = match ordo_exec::spacedb::all(pool).await {
            Ok(a) => a.into_iter().map(|(s, _)| s).collect::<Vec<_>>(),
            Err(e) if e.is_missing_table() => Vec::new(),
            Err(e) => return Err(e),
        };
        for space in own {
            let Ok(p) = pools.resolve(&space).await else {
                continue;
            };
            let only = format!(" AND \"$space\" = {}", sql_lit(&space.0));
            grants.extend(pairs(&p, &format!("{GRANTS_SQL}{now}{only}")).await);
            members.extend(pairs(&p, &format!("{members_of}{only}")).await);
        }
    }
    let mut by_person: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (space, person) in members {
        by_person.entry(person).or_default().insert(space);
    }
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (a, person) in grants {
        for b in by_person.get(&person).into_iter().flatten() {
            if *b != a {
                out.entry(a.clone()).or_default().insert(b.clone());
                out.entry(b.clone()).or_default().insert(a.clone());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(id: &str, name: &str, members: Option<i64>, personal: bool) -> Facts {
        let mut values = BTreeMap::new();
        values.insert("tenant".to_string(), Fact::Text(id.into()));
        values.insert("name".into(), Fact::Text(name.into()));
        values.insert("version".into(), Fact::Text("0.1.0".into()));
        values.insert(
            "members".into(),
            members.map(Fact::Int).unwrap_or(Fact::Null),
        );
        values.insert("personal".into(), Fact::Bool(personal));
        // Usage: a's the busiest, c's never counted.
        let used = match id {
            "a" => Some(900),
            "b" => Some(10),
            "d" => Some(40),
            _ => None,
        };
        values.insert(
            "requests30d".into(),
            used.map(Fact::Int).unwrap_or(Fact::Null),
        );
        values.insert("rows.task".into(), Fact::Int(id.len() as i64 * 7));
        Facts {
            tenant: id.into(),
            values,
        }
    }

    #[test]
    fn a_query_selects_over_the_facts_in_its_order() {
        let all = vec![
            tenant("a", "Acme", Some(9), false),
            tenant("b", "Bolt", Some(2), false),
            tenant("c", "Acme East", None, true),
            tenant("d", "Dune", Some(5), false),
        ];
        let q = Query::parse("members < 6", Some("members asc")).unwrap();
        assert!(q.uses_members());
        assert_eq!(q.select(all.clone()), ["b", "d"]);
        let q = Query::parse("name contains \"Acme\"", Some("tenant desc")).unwrap();
        assert_eq!(q.select(all.clone()), ["c", "a"]);
        let q = Query::parse("not personal and members is not null", Some("members desc")).unwrap();
        assert_eq!(q.select(all.clone()), ["a", "d", "b"]);
        let q = Query::parse("version != \"0.2.0\"", None).unwrap();
        assert_eq!(q.select(all.clone()), ["a", "b", "c", "d"]);
        // "The 2 least used": an order and a limit. Never counted sorts last.
        let q = Query::parse("true", Some("requests30d asc"))
            .unwrap()
            .limited(Some(2));
        assert!(q.needs().usage);
        assert_eq!(q.select(all.clone()), ["b", "d"]);
        // "Most rows of an entity".
        let q = Query::parse("rows.task > 0", Some("rows.task desc, tenant asc")).unwrap();
        assert_eq!(q.needs().rows.iter().collect::<Vec<_>>(), ["task"]);
        assert_eq!(q.select(all), ["a", "b", "c", "d"]);
    }

    #[test]
    fn a_query_is_refused_naming_the_facts() {
        let e = Query::parse("seats > 3", None).unwrap_err();
        assert!(
            e.contains("not a tenant fact") && e.contains("members"),
            "{e}"
        );
        assert!(Query::parse("members > \"x\"", None).is_err());
        assert!(Query::parse("members + 1 > 3", None).is_err());
        assert!(Query::parse("name matches \"a.*\"", None).is_err());
        assert!(Query::parse("members > $n", None).is_err());
        let e = Query::parse("members > 1", Some("size desc")).unwrap_err();
        assert!(e.contains("not a tenant fact"), "{e}");
        assert!(Query::parse("members >", None).is_err());
        assert!(Query::parse("rows.task.x > 1", None).is_err());
        assert!(Query::parse("rows.\"x\" > 1", None).is_err());
        assert!(Query::parse("usage.task > 1", None).is_err());
        // Every fact parses: Ordo names are lowerCamel.
        for (name, ty, _) in FACTS {
            let cond = match ty {
                FactTy::Bool => format!("{name} or not {name}"),
                FactTy::Int => format!("{name} >= 0"),
                FactTy::Text => format!("{name} != \"x\""),
            };
            assert!(Query::parse(&cond, Some(name)).is_ok(), "{cond}");
        }
    }

    #[test]
    fn rates_read_and_print_and_judge() {
        assert_eq!(parse_rate("1%"), Some(100));
        assert_eq!(parse_rate("0.5"), Some(50));
        assert_eq!(parse_rate("2.25%"), Some(225));
        assert_eq!(parse_rate("0"), None);
        assert_eq!(parse_rate("101"), None);
        assert_eq!(parse_rate("x"), None);
        assert_eq!(format_rate(100), "1%");
        assert_eq!(format_rate(50), "0.5%");
        assert_eq!(format_rate(225), "2.25%");
        assert!(!unhealthy(0, 0, 100));
        assert!(!unhealthy(1000, 9, 100));
        assert!(unhealthy(1000, 10, 100));
        assert!(unhealthy(3, 3, 100));
    }

    #[test]
    fn a_channel_is_the_stream_or_a_mail() {
        assert_eq!(check_channel("stream").unwrap(), "stream");
        assert_eq!(
            check_channel("mail:ops@example.com").unwrap(),
            "mail:ops@example.com"
        );
        assert!(check_channel("mail:nope").is_err());
        assert!(check_channel("slack").is_err());
    }
}
