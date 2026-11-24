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

use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_syntax::ast::{CmpOp, Condition, Expr, TextOp};

// ── health ───────────────────────────────────────────────────────────────

/// `rollout.health_error_rate`'s default, in hundredths of a percent: 1%.
pub const HEALTH_ERROR_RATE_DEFAULT: i64 = 100;

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
/// signal, and reads as healthy.
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
/// registry. A tenant edits none of them.
pub const FACTS: [(&str, FactTy, &str); 9] = [
    ("tenant", FactTy::Text, "the tenant's id"),
    ("name", FactTy::Text, "the tenant's name"),
    ("version", FactTy::Text, "the version it is on"),
    (
        "created_at",
        FactTy::Int,
        "when it was made, in seconds since 1970",
    ),
    ("age_days", FactTy::Int, "whole days since it was made"),
    (
        "members",
        FactTy::Int,
        "its members; none for a tenant with its own database",
    ),
    ("personal", FactTy::Bool, "a person's own tenant"),
    ("own_database", FactTy::Bool, "it has a database of its own"),
    ("paused", FactTy::Bool, "it is paused"),
];

fn fact_ty(name: &str) -> Option<(&'static str, FactTy)> {
    FACTS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(n, t, _)| (*n, *t))
}

fn fact_names() -> String {
    FACTS
        .iter()
        .map(|(n, _, _)| *n)
        .collect::<Vec<_>>()
        .join(", ")
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
    values: BTreeMap<&'static str, Fact>,
}

impl Facts {
    pub fn get(&self, name: &str) -> &Fact {
        self.values.get(name).unwrap_or(&Fact::Null)
    }
}

/// A query as checked: which tenants, in which order.
#[derive(Debug, Clone)]
pub struct Query {
    cond: Condition,
    order: Vec<(&'static str, bool)>,
    uses_members: bool,
}

/// One side of a comparison, checked.
enum Side {
    Fact(&'static str, FactTy),
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
        Expr::Path { segments, .. } if segments.len() == 1 => {
            let name = segments[0].name.as_str();
            fact_ty(name)
                .map(|(n, t)| Side::Fact(n, t))
                .ok_or_else(|| format!("`{name}` is not a tenant fact; there are {}", fact_names()))
        }
        Expr::Path { segments, .. } => Err(format!(
            "`{}` is a path; a query reads one tenant fact by name: {}",
            segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join("."),
            fact_names()
        )),
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

fn check(c: &Condition, members: &mut bool) -> Result<(), String> {
    let note = |s: &Side, members: &mut bool| {
        if matches!(s, Side::Fact("members", _)) {
            *members = true;
        }
    };
    match c {
        Condition::Compare { op, lhs, rhs, .. } => {
            let (l, r) = (side(lhs)?, side(rhs)?);
            note(&l, members);
            note(&r, members);
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
            note(&s, members);
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
                    "`matches` is not offered over tenant facts; use `contains` or \
                            `starts`"
                        .into(),
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
            note(&s, members);
            Ok(())
        }
        Condition::BareBool(e) => match side(e)? {
            Side::Fact(_, FactTy::Bool) | Side::Lit(Fact::Bool(_)) => Ok(()),
            _ => Err("a condition on its own is a true or false fact, like `personal`".into()),
        },
        Condition::And { items, .. } | Condition::Or { items, .. } => {
            items.iter().try_for_each(|i| check(i, members))
        }
        Condition::Not { inner, .. } => check(inner, members),
        Condition::Since { .. } | Condition::Under { .. } | Condition::Above { .. } => Err(
            "`since`, `under` and `above` are not offered over tenant facts; compare \
             `age_days` or `created_at`"
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
            let (a, b) = (l.value(f), r.value(f));
            let ord = match (a, b) {
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
    /// **Check a query**: an Ordo condition over [`FACTS`], and an order,
    /// `<fact> [asc|desc]`, comma separated. Refused naming what is wrong
    /// and the facts there are.
    pub fn parse(select: &str, order: Option<&str>) -> Result<Query, String> {
        let cond = ordo_syntax::parser::parse_condition(select.trim())
            .map_err(|e| format!("it does not parse: {}", e.message))?;
        let mut uses_members = false;
        check(&cond, &mut uses_members)?;
        let mut by = Vec::new();
        for part in order
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            let mut words = part.split_whitespace();
            let name = words.next().unwrap_or_default();
            let (name, _) = fact_ty(name).ok_or_else(|| {
                format!(
                    "the order names `{name}`, which is not a tenant fact; there are {}",
                    fact_names()
                )
            })?;
            let desc = match words.next() {
                None | Some("asc") => false,
                Some("desc") => true,
                Some(w) => return Err(format!("the order says `{w}`; it is `asc` or `desc`")),
            };
            if words.next().is_some() {
                return Err("the order is `<fact> [asc|desc]`, comma separated".into());
            }
            if name == "members" {
                uses_members = true;
            }
            by.push((name, desc));
        }
        Ok(Query {
            cond,
            order: by,
            uses_members,
        })
    }

    pub fn uses_members(&self) -> bool {
        self.uses_members
    }

    pub fn selects(&self, f: &Facts) -> bool {
        eval(&self.cond, f)
    }

    /// The tenants it selects, in its order, then by id. A null sorts
    /// last either way.
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
        chosen.into_iter().map(|f| f.tenant).collect()
    }
}

/// **The facts of every serving tenant**, read from the control database
/// only. `members` is read when the query uses it; a tenant with its own
/// database has none here.
pub async fn facts(pool: &Pool, with_members: bool, now: u64) -> Result<Vec<Facts>, PoolError> {
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
    let mut members: Option<BTreeMap<String, i64>> = None;
    if with_members {
        members = match pool
            .rows(
                "SELECT \"$space\", count(*) FROM \"members\" WHERE \"kind\" = 'principal' \
                 GROUP BY 1",
            )
            .await
        {
            Ok(r) => Some(
                r.into_iter()
                    .filter_map(|r| {
                        Some((r.first()?.clone()?, r.get(1)?.clone()?.parse::<i64>().ok()?))
                    })
                    .collect(),
            ),
            Err(e) if e.is_missing_table() => None,
            Err(e) => return Err(e),
        };
    }
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let s = |i: usize| r.get(i).cloned().flatten();
            let tenant = s(0)?;
            let created = s(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
            let own_db = own.contains(&tenant);
            let mut values = BTreeMap::new();
            values.insert("tenant", Fact::Text(tenant.clone()));
            values.insert("name", Fact::Text(s(1).unwrap_or_default()));
            values.insert("version", s(2).map(Fact::Text).unwrap_or(Fact::Null));
            values.insert("created_at", Fact::Int(created));
            values.insert(
                "age_days",
                Fact::Int((now as i64).saturating_sub(created).max(0) / 86_400),
            );
            values.insert(
                "members",
                match (&members, own_db) {
                    (Some(m), false) => Fact::Int(m.get(&tenant).copied().unwrap_or(0)),
                    _ => Fact::Null,
                },
            );
            values.insert(
                "personal",
                Fact::Bool(matches!(s(4).as_deref(), Some("t" | "true"))),
            );
            values.insert("own_database", Fact::Bool(own_db));
            values.insert("paused", Fact::Bool(s(5).as_deref() == Some("paused")));
            Some(Facts { tenant, values })
        })
        .collect())
}

// ── share partners ──────────────────────────────────────────────────────

/// **Every tenant's share partners**, both ways: a tenant that holds an
/// open capability grant to a person, and each tenant that person is a
/// member of. Read from the control database; nothing when the tables are
/// not there.
pub async fn partners(
    pool: &Pool,
    now: u64,
) -> Result<BTreeMap<String, BTreeSet<String>>, PoolError> {
    let rows = match pool
        .rows(&format!(
            "SELECT DISTINCT g.\"$space\", m.\"$space\" FROM \"$capabilitygrant\" g \
             JOIN \"members\" m ON m.\"principal\" = g.\"principal\" AND m.\"kind\" = 'principal' \
             WHERE g.\"revoked_at\" IS NULL AND g.\"expires\" > {now} \
               AND m.\"$space\" <> g.\"$space\""
        ))
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_missing_table() => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in rows {
        let (Some(Some(a)), Some(Some(b))) = (r.first().cloned(), r.get(1).cloned()) else {
            continue;
        };
        out.entry(a.clone()).or_default().insert(b.clone());
        out.entry(b).or_default().insert(a);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(id: &str, name: &str, members: Option<i64>, personal: bool) -> Facts {
        let mut values = BTreeMap::new();
        values.insert("tenant", Fact::Text(id.into()));
        values.insert("name", Fact::Text(name.into()));
        values.insert("version", Fact::Text("0.1.0".into()));
        values.insert("members", members.map(Fact::Int).unwrap_or(Fact::Null));
        values.insert("personal", Fact::Bool(personal));
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
