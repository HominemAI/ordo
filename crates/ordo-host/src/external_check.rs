//! External entities, slice C (ruled 2026-11-15, `external-entities`): the
//! check of each translation against the live table, the list of what is in
//! the database and not Ordo's, and the watcher that keeps the first current.
//!
//! # What the check compares
//!
//! For each declared `external`, read from the database the program applies
//! to (the deployment's own, or one tenant's):
//!
//! | Found | Result |
//! |---|---|
//! | the table is not there | `external_missing`, a refusal |
//! | Ordo's role lacks `USAGE` on the schema or `SELECT` on the table | `external_no_grant`, a refusal naming the `GRANT` |
//! | a mapped column is gone, has another type, or admits null where the file says it never does | `external_changed`, a refusal naming each column |
//! | a column the translation does not read | a note, never a refusal |
//!
//! The comparison is by type family, as `docs/migration.md` section 2a draws
//! them: `varchar(120)` satisfies `text`, `integer` satisfies `int`. A
//! `decimal(s)` field needs `numeric` at exactly that scale. Nothing here
//! writes. A refusal is a line that says what the deployment owner can do.
//!
//! # Where it runs
//!
//! [`check`] is called from `schema_apply::plan_with`, the one place a plan
//! is read, so `ordo plan`, `ordo apply`, `ordo evolve --run`
//! and the tenant catch-up all see it. [`ExternalGate`] is the runtime half:
//! a timer re-runs the check on the deployment's database, and while an
//! entity is down every op that reads it answers `external_changed`. When
//! the table comes back unchanged, the next tick lifts the hold.
//!
//! Tenant databases are checked when a program is applied to them. The
//! timer watches the deployment's own database, for every version the node
//! serves (`plan-evolution-federation.md` step F3): a hold found for one
//! version holds only the tenants that version serves.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::future::BoxFuture;
use ordo_api::{ErrorCode, ExecError, ExecOutcome, OpExecutor, OpSpec};
use ordo_exec::migrate;
use ordo_exec::Pool;
use ordo_schema::{Entity, FieldTy, Schema};
use ordo_syntax::{ExternalKeyTy, ScalarTy};

/// The setting that names how often the watcher re-reads the tables.
pub const WATCH_ENV: &str = "ORDO_EXTERNAL_WATCH_SECS";
/// Five minutes.
pub const WATCH_DEFAULT_SECS: u64 = 300;

/// [`WATCH_ENV`], or the default when unset. A value that is not a whole
/// number of seconds from 1 is refused at boot, naming the setting.
pub fn watch_interval_from_env() -> Result<Duration, String> {
    match std::env::var(WATCH_ENV) {
        Err(_) => Ok(Duration::from_secs(WATCH_DEFAULT_SECS)),
        Ok(v) if v.trim().is_empty() => Ok(Duration::from_secs(WATCH_DEFAULT_SECS)),
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(n) if n >= 1 => Ok(Duration::from_secs(n)),
            _ => Err(format!(
                "{WATCH_ENV} is `{v}`: it is a whole number of seconds, 1 or more. Set \
                 {WATCH_ENV}={WATCH_DEFAULT_SECS}, the default, or unset it"
            )),
        },
    }
}

/// What kind of problem one external has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    /// The table is not in this database.
    Missing,
    /// Ordo's role cannot read the table.
    NoGrant,
    /// A mapped column is gone, retyped, or nullable against the file.
    Changed,
    /// A peer's schema holds no Ordo deployment, so `$caller` cannot be
    /// resolved there.
    PeerNotOrdo,
}

impl ProblemKind {
    /// The greppable class.
    pub fn class(self) -> &'static str {
        match self {
            ProblemKind::Missing => "external_missing",
            ProblemKind::NoGrant => "external_no_grant",
            ProblemKind::Changed => "external_changed",
            ProblemKind::PeerNotOrdo => "peer_not_ordo",
        }
    }
}

/// One external that cannot be read as the program says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub entity: String,
    pub kind: ProblemKind,
    /// What is wrong and what to do, in short sentences.
    pub detail: String,
}

impl Problem {
    /// `external_changed on `customers`: …`, the shape a plan lists.
    pub fn line(&self) -> String {
        format!(
            "{} on `{}`: {}",
            self.kind.class(),
            self.entity,
            self.detail
        )
    }
}

/// Everything one pass learned about a program's externals in one database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExternalReport {
    pub problems: Vec<Problem>,
    /// What was taken: columns the translation does not read.
    pub notes: Vec<String>,
    /// What the database holds that is not Ordo's and not a declared
    /// external (27j, "present, not Ordo's"). One line each.
    pub present: Vec<String>,
}

impl ExternalReport {
    /// The refusals, one line each.
    pub fn refusals(&self) -> Vec<String> {
        self.problems.iter().map(Problem::line).collect()
    }

    /// The notes and the present list, as the lines a plan prints.
    pub fn display_notes(&self) -> Vec<String> {
        let mut out = self.notes.clone();
        if !self.present.is_empty() {
            out.push("present, not Ordo's (nothing is changed):".to_string());
            out.extend(self.present.iter().map(|p| format!("  {p}")));
        }
        out
    }
}

/// The most "present, not Ordo's" lines one plan prints.
const PRESENT_CAP: usize = 20;

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Does the live type `live` (Postgres' own spelling) satisfy the declared
/// field type?
fn accepts(ty: &FieldTy, live: &str, schema: &Schema) -> bool {
    let lower = live.to_ascii_lowercase();
    let base = lower.split('(').next().unwrap_or(&lower).trim();
    let int = matches!(base, "smallint" | "integer" | "bigint");
    let text = matches!(
        base,
        "text" | "character varying" | "character" | "citext" | "name"
    );
    match ty {
        // A ref holds the target's typed id over the target's raw key, so the
        // column must be of the target's key type.
        FieldTy::Ref { target, .. } => {
            match schema.entity(target).and_then(|t| t.external.as_ref()) {
                Some(src) => accepts_key(src.key_ty, live),
                None => int || text || base == "uuid",
            }
        }
        FieldTy::Scalar(s) => match s {
            ScalarTy::Text | ScalarTy::Uri => text,
            ScalarTy::Int | ScalarTy::Money(_) | ScalarTy::Duration(_) => int,
            ScalarTy::Bool => base == "boolean",
            ScalarTy::Date => base == "date",
            ScalarTy::Datetime => {
                matches!(
                    base,
                    "timestamp with time zone" | "timestamp without time zone"
                )
            }
            ScalarTy::Decimal(places) => {
                base == "numeric"
                    && lower
                        .split_once('(')
                        .and_then(|(_, r)| r.strip_suffix(')'))
                        .and_then(|inner| match inner.split_once(',') {
                            Some((_, s)) => s.trim().parse::<u8>().ok(),
                            None => Some(0),
                        })
                        == Some(*places)
            }
            ScalarTy::Json => matches!(base, "json" | "jsonb"),
            // A blob is Ordo's own handle, an id is Ordo's own key. Neither
            // reads a foreign column.
            ScalarTy::Blob | ScalarTy::Id => false,
        },
    }
}

fn accepts_key(key: ExternalKeyTy, live: &str) -> bool {
    let lower = live.to_ascii_lowercase();
    let base = lower.split('(').next().unwrap_or(&lower).trim();
    match key {
        ExternalKeyTy::Int => matches!(base, "smallint" | "integer" | "bigint"),
        ExternalKeyTy::Text => {
            matches!(base, "text" | "character varying" | "character" | "citext")
        }
        ExternalKeyTy::Uuid => base == "uuid",
    }
}

fn spelled(ty: &FieldTy, schema: &Schema) -> String {
    match ty {
        FieldTy::Scalar(s) => s.as_str().to_string(),
        FieldTy::Ref { target, .. } => {
            match schema.entity(target).and_then(|t| t.external.as_ref()) {
                Some(src) => format!("a ref to `{target}`, whose key is {}", src.key_ty.as_str()),
                None => format!("a ref to `{target}`"),
            }
        }
    }
}

/// Compare one external with its live table.
async fn check_one(
    pool: &Pool,
    e: &Entity,
    schema: &Schema,
    report: &mut ExternalReport,
) -> Result<(), String> {
    let Some(src) = &e.external else {
        return Ok(());
    };
    // The schema this environment reads: a peer's resolved schema, or a
    // plain external's swap (`ordo_exec::peer`).
    let pg = src.pg_schema();
    let table = format!("{pg}.{}", src.table);
    let regenerate = format!(
        "Regenerate it with `ordo introspect --external {table}`, review the change and apply it"
    );
    let db = |what: &str, err: ordo_exec::PoolError| format!("reading {what} of {table}: {err}");
    let Some(live) = migrate::table_columns(pool, &pg, &src.table)
        .await
        .map_err(|err| db("the columns", err))?
    else {
        report.problems.push(Problem {
            entity: e.name.clone(),
            kind: ProblemKind::Missing,
            detail: format!(
                "table `{table}` is not in this database. Create it, or remove `external {}` \
                 from the program",
                e.name
            ),
        });
        return Ok(());
    };
    if let Some(a) = migrate::access_to(pool, &pg, &src.table)
        .await
        .map_err(|err| db("the grants", err))?
    {
        // **A column the read app stopped writing** (owner ruling
        // 2026-11-23, Q1). A peer that left a mapped column behind grants
        // its table by column, without it, so the table reads as not
        // granted whole while the other mapped columns are. The reader is
        // held, and the refusal names each column it lost.
        let mut column_granted = false;
        if let (Some(p), true, false) = (src.peer.as_ref(), a.schema_usage, a.select) {
            let mut mapped: Vec<&str> = vec![src.key_column.as_str()];
            mapped.extend(src.columns.iter().map(|(_, c)| c.as_str()));
            let lost = migrate::columns_not_granted(pool, &pg, &src.table, &mapped)
                .await
                .map_err(|err| db("the column grants", err))?;
            column_granted = lost.is_empty();
            if !lost.is_empty() && lost.len() < mapped.len() {
                let named = lost
                    .iter()
                    .map(|c| format!("column `{c}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let (is, it) = if lost.len() == 1 {
                    ("is", "it")
                } else {
                    ("are", "them")
                };
                report.problems.push(Problem {
                    entity: e.name.clone(),
                    kind: ProblemKind::Changed,
                    detail: format!(
                        "{named} {is} no longer written by `{}`: a version of it stopped \
                         declaring {it}, and its grant to this deployment ended. {regenerate}",
                        p.name
                    ),
                });
                return Ok(());
            }
        }
        if let Some(p) = src
            .peer
            .as_ref()
            .filter(|_| !column_granted && (!a.schema_usage || !a.select))
        {
            // A peer's table: the deployment being read grants it, by its
            // `reader` line (owner ruling 2026-11-18).
            report.problems.push(Problem {
                entity: e.name.clone(),
                kind: ProblemKind::NoGrant,
                detail: format!(
                    "role `{}` cannot read `{table}`. Add `reader <name> reads {}` to {}'s \
                     definition file, set ORDO_READER_<NAME>_ROLE={} in its settings, and \
                     apply or restart it",
                    a.role, src.table, p.name, a.role
                ),
            });
            return Ok(());
        }
        if !column_granted && (!a.schema_usage || !a.select) {
            let mut grants = Vec::new();
            if !a.schema_usage {
                grants.push(format!(
                    "GRANT USAGE ON SCHEMA {} TO {};",
                    quoted(&pg),
                    quoted(&a.role)
                ));
            }
            if !a.select {
                grants.push(format!(
                    "GRANT SELECT ON {}.{} TO {};",
                    quoted(&pg),
                    quoted(&src.table),
                    quoted(&a.role)
                ));
            }
            report.problems.push(Problem {
                entity: e.name.clone(),
                kind: ProblemKind::NoGrant,
                detail: format!(
                    "role `{}` cannot read `{table}`. Run {}",
                    a.role,
                    grants.join(" ")
                ),
            });
            return Ok(());
        }
    }

    let mut wrong: Vec<String> = Vec::new();
    let mut read: Vec<&str> = vec![src.key_column.as_str()];
    match live.get(&src.key_column) {
        None => wrong.push(format!("key column `{}` is gone", src.key_column)),
        Some(c) => {
            if !accepts_key(src.key_ty, &c.ty) {
                wrong.push(format!(
                    "key column `{}` is `{}`, and the file reads it as {}",
                    src.key_column,
                    c.ty,
                    src.key_ty.as_str()
                ));
            } else if c.nullable {
                wrong.push(format!(
                    "key column `{}` now admits null, and a key never does",
                    src.key_column
                ));
            }
        }
    }
    for (field, column) in &src.columns {
        read.push(column.as_str());
        let Some(f) = e.field(field) else {
            continue;
        };
        match live.get(column) {
            None => wrong.push(format!("column `{column}` is gone")),
            Some(c) if !accepts(&f.ty, &c.ty, schema) => wrong.push(format!(
                "column `{column}` is `{}`, and the file reads it as {}",
                c.ty,
                spelled(&f.ty, schema)
            )),
            Some(c) if c.nullable && !f.optional => wrong.push(format!(
                "column `{column}` admits null, and the file says `{field}` never does. \
                 Write `{field}` with a `?`"
            )),
            Some(_) => {}
        }
    }
    if !wrong.is_empty() {
        report.problems.push(Problem {
            entity: e.name.clone(),
            kind: ProblemKind::Changed,
            detail: format!("{}. {regenerate}", wrong.join("; ")),
        });
        return Ok(());
    }
    let unused: Vec<String> = live
        .keys()
        .filter(|c| !read.contains(&c.as_str()))
        .map(|c| format!("`{c}`"))
        .collect();
    if !unused.is_empty() {
        report.notes.push(format!(
            "note: `{table}` has {} the translation of `{}` does not read: {}",
            if unused.len() == 1 {
                "a column"
            } else {
                "columns"
            },
            e.name,
            unused.join(", ")
        ));
    }
    Ok(())
}

/// Compare every external of `schema` with the database `pool` reads, and
/// list what that database holds that is not Ordo's. Read-only.
pub async fn check(pool: &Pool, schema: &Schema) -> Result<ExternalReport, String> {
    let mut report = ExternalReport::default();
    for e in &schema.externals {
        check_one(pool, e, schema, &mut report).await?;
    }
    if let Err(refusal) = ordo_exec::peer::check(pool, schema).await {
        if let ordo_exec::peer::SchemaRefusal::Database(e) = refusal {
            return Err(format!("reading the peer schemas: {e}"));
        }
        report.problems.push(Problem {
            entity: schema
                .externals
                .iter()
                .find(|e| e.external.as_ref().is_some_and(|x| x.peer.is_some()))
                .map(|e| e.name.clone())
                .unwrap_or_default(),
            kind: ProblemKind::PeerNotOrdo,
            detail: refusal.to_string(),
        });
    }
    report.present = present_not_ordos(pool, schema).await?;
    Ok(report)
}

/// Tables and schemas the database holds that are not Ordo's and not a
/// declared external (27j). Ordo's are the program's entities and the
/// platform's `$` tables in the deployment's own schema (`Pool::schema`,
/// `public` unless `ORDO_PG_SCHEMA` names another). Another deployment's
/// schema in the same database is listed: its tables are not this
/// program's.
pub async fn present_not_ordos(pool: &Pool, schema: &Schema) -> Result<Vec<String>, String> {
    let (schemas, tables) = migrate::schemas_and_tables(pool)
        .await
        .map_err(|e| format!("reading what the database holds: {e}"))?;
    let declared = |s: &str, t: &str| {
        schema.externals.iter().any(|e| {
            e.external
                .as_ref()
                .is_some_and(|x| x.pg_schema() == s && x.table == t)
        })
    };
    let mut lines = Vec::new();
    let mut used_schemas: Vec<&str> = Vec::new();
    for (s, t) in &tables {
        if ordo_exec::shape::is_ordo_schema(s) {
            continue;
        }
        if s == pool.schema() {
            if t.starts_with('$') || schema.native(t).is_some() {
                continue;
            }
        } else {
            used_schemas.push(s);
            if declared(s, t) {
                continue;
            }
        }
        lines.push(format!("table `{s}.{t}`"));
    }
    for s in &schemas {
        if !used_schemas.contains(&s.as_str()) && !ordo_exec::shape::is_ordo_schema(s) {
            lines.push(format!("schema `{s}`, with no tables"));
        }
    }
    if lines.len() > PRESENT_CAP {
        let more = lines.len() - PRESENT_CAP;
        lines.truncate(PRESENT_CAP);
        lines.push(format!("and {more} more"));
    }
    Ok(lines)
}

// ── the watcher ──────────────────────────────────────────────────────────

/// The hold on the entities that are down, and the timer's read of them.
///
/// While an entity is held, every op that reads it answers
/// `external_changed`, naming the table, the column and the fix. The hold is
/// lifted by the first pass that finds the table as the program says.
///
/// **Every served version is checked** (`plan-evolution-federation.md`
/// step F3, `plan-versions.md` §1.15): each pass reads the externals of
/// every version the node serves, and a hold found for one version holds
/// only the tenants that version serves.
pub struct ExternalGate {
    /// What each served fixture's last pass found, by the fixture's
    /// identity. A hold is never applied to another fixture: applying a
    /// program checks its translation against the tables (`plan_with`), so
    /// a newly served fixture starts with no hold and the next pass reads
    /// it again.
    holds: RwLock<HashMap<usize, BTreeMap<String, String>>>,
    /// The active version's fixture.
    active: Source,
    /// Every fixture this node serves, the active one first.
    served: Served,
    /// The fixture that serves one tenant.
    for_space: ForSpace,
    reads: RwLock<HashMap<usize, Reads>>,
}

/// The fixture the deployment serves now.
pub type Source = Arc<dyn Fn() -> Arc<ordo_conformance::Fixture> + Send + Sync>;
/// Every version a node serves, by name, the active one first.
pub type Served = Arc<dyn Fn() -> Vec<(String, Arc<ordo_conformance::Fixture>)> + Send + Sync>;
/// The fixture that serves one tenant.
pub type ForSpace = Arc<dyn Fn(&str) -> Arc<ordo_conformance::Fixture> + Send + Sync>;

/// The externals each op of one fixture reads, worked out once.
struct Reads {
    fixture: Arc<ordo_conformance::Fixture>,
    by_op: Arc<BTreeMap<String, Vec<String>>>,
}

fn identity(f: &Arc<ordo_conformance::Fixture>) -> usize {
    Arc::as_ptr(f) as usize
}

/// Every quoted name in an op's debug form: entity, field and op names.
/// What an op's source mentions, for an op the bind cannot settle.
fn mentioned(op: &impl std::fmt::Debug) -> Vec<String> {
    let text = format!("{op:?}");
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find('"') {
        rest = &rest[i + 1..];
        let Some(j) = rest.find('"') else { break };
        out.push(rest[..j].to_string());
        rest = &rest[j + 1..];
    }
    out
}

/// The externals one op reaches, from the registry and never from a failed
/// bind. A read the generic bind settles is answered by the planner's own
/// `externals_read`. One it cannot (a `$caller` used as a value) and every
/// write are read from the names the op mentions, through the ops it calls.
/// A read also follows refs, because a hop reaches the target; a write does
/// not, because naming a ref in an assignment reads nothing (the write path
/// checks the prefix only).
fn reads_of(f: &ordo_conformance::Fixture, op: &str) -> Vec<String> {
    let Some(decl) = f.registry.get(op) else {
        return Vec::new();
    };
    let is_write = matches!(decl.body, ordo_syntax::OpBody::Write(_));
    if !is_write {
        if let Ok(q) =
            ordo_sema::bind_op_generic(&f.schema, &f.registry, op, ordo_sema::Clock::CORPUS)
        {
            return ordo_plan::externals_read(&q);
        }
    }
    // What the op mentions. A write mentions only its conditions and the
    // values it computes, never the field it assigns: naming a ref there
    // reads nothing, because the write path checks the prefix only.
    let mut seen_ops: Vec<String> = vec![op.to_string()];
    let mut names: Vec<String> = Vec::new();
    let mut todo = vec![op.to_string()];
    while let Some(o) = todo.pop() {
        let Some(d) = f.registry.get(&o) else {
            continue;
        };
        let found = match &d.body {
            ordo_syntax::OpBody::Read(p) => mentioned(p),
            ordo_syntax::OpBody::Write(stmts) => stmts
                .iter()
                .flat_map(|st| match st {
                    ordo_syntax::WriteStmt::Set { cond, fields, .. } => {
                        let mut v = mentioned(cond);
                        for (_, e) in fields {
                            v.extend(mentioned(e));
                        }
                        v
                    }
                    ordo_syntax::WriteStmt::Del { cond, .. } => mentioned(cond),
                })
                .collect(),
        };
        for n in found {
            if f.registry.get(&n).is_some() && !seen_ops.contains(&n) {
                seen_ops.push(n.clone());
                todo.push(n.clone());
            }
            names.push(n);
        }
    }
    let mut reached: Vec<String> = names
        .iter()
        .filter(|n| f.schema.entity(n).is_some())
        .cloned()
        .collect();
    // A read follows every ref of an entity it names, as a guard or a
    // projection may hop. A write cannot hop in a condition.
    if !is_write {
        let mut i = 0;
        while i < reached.len() {
            if let Some(e) = f.schema.entity(&reached[i]) {
                for fld in &e.fields {
                    if let ordo_schema::FieldTy::Ref { target, .. } = &fld.ty {
                        if !reached.contains(target) {
                            reached.push(target.clone());
                        }
                    }
                }
            }
            i += 1;
        }
    }
    reached.retain(|n| f.schema.is_external(n));
    reached.sort();
    reached.dedup();
    reached
}

impl ExternalGate {
    /// One program for every tenant: `source` returns the fixture the
    /// deployment serves now.
    pub fn new(source: Source) -> Arc<ExternalGate> {
        let served = source.clone();
        let for_space = source.clone();
        Arc::new(ExternalGate {
            holds: RwLock::new(HashMap::new()),
            active: source,
            served: Arc::new(move || vec![(String::new(), served())]),
            for_space: Arc::new(move |_| for_space()),
            reads: RwLock::new(HashMap::new()),
        })
    }

    /// **Every served version** (step F3): `active` returns the active
    /// version's fixture, `served` every fixture this node serves, and
    /// `for_space` the one that serves a tenant.
    pub fn per_version(active: Source, served: Served, for_space: ForSpace) -> Arc<ExternalGate> {
        Arc::new(ExternalGate {
            holds: RwLock::new(HashMap::new()),
            active,
            served,
            for_space,
            reads: RwLock::new(HashMap::new()),
        })
    }

    /// The entities held now in any served version, with the line that
    /// says why.
    pub fn down(&self) -> BTreeMap<String, String> {
        let holds = self.holds.read().unwrap_or_else(|e| e.into_inner());
        let mut out = BTreeMap::new();
        for down in holds.values() {
            for (k, v) in down {
                out.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        out
    }

    /// The entities held for the version that serves `space`.
    pub fn down_in(&self, space: &str) -> BTreeMap<String, String> {
        let f = (self.for_space)(space);
        self.holds
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&identity(&f))
            .cloned()
            .unwrap_or_default()
    }

    /// The externals each op reads, once per fixture (keyed by the fixture's
    /// identity, kept alive here so the identity cannot be reused).
    fn reads_for(&self, f: &Arc<ordo_conformance::Fixture>) -> Arc<BTreeMap<String, Vec<String>>> {
        if let Some(r) = self
            .reads
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&identity(f))
        {
            if Arc::ptr_eq(&r.fixture, f) {
                return r.by_op.clone();
            }
        }
        let by_op: BTreeMap<String, Vec<String>> = f
            .registry
            .names()
            .map(|n| (n.clone(), reads_of(f, n)))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let by_op = Arc::new(by_op);
        self.reads
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                identity(f),
                Reads {
                    fixture: f.clone(),
                    by_op: by_op.clone(),
                },
            );
        by_op
    }

    /// One pass: read the tables every served version declares and set
    /// each version's hold to what is wrong with them. Returns the lines
    /// now held, each version's once, naming the version when more than
    /// one is served.
    pub async fn watch_once(&self, pool: &Pool) -> Result<Vec<String>, String> {
        let served = (self.served)();
        let many = served.len() > 1;
        let mut holds: HashMap<usize, BTreeMap<String, String>> = HashMap::new();
        let mut lines = Vec::new();
        for (version, fixture) in &served {
            if fixture.schema.externals.is_empty() || holds.contains_key(&identity(fixture)) {
                continue;
            }
            // The catalog read only: the present list is for a plan.
            let mut report = ExternalReport::default();
            for e in &fixture.schema.externals {
                check_one(pool, e, &fixture.schema, &mut report).await?;
            }
            for l in report.refusals() {
                lines.push(if many { format!("{version}: {l}") } else { l });
            }
            let down: BTreeMap<String, String> = report
                .problems
                .into_iter()
                .map(|p| (p.entity.clone(), p.line()))
                .collect();
            if !down.is_empty() {
                holds.insert(identity(fixture), down);
            }
        }
        *self.holds.write().unwrap_or_else(|e| e.into_inner()) = holds;
        // A version no longer served keeps nothing here.
        let keep: std::collections::HashSet<usize> =
            served.iter().map(|(_, f)| identity(f)).collect();
        self.reads
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|k, _| keep.contains(k));
        Ok(lines)
    }

    /// The refusal for `op` in the active version, when it reads a held
    /// entity.
    pub fn refusal(&self, op: &str) -> Option<ExecError> {
        self.refusal_for(op, &(self.active)())
    }

    /// The refusal for `op` in the version that serves `space`, when it
    /// reads an entity held for that version. A hold found for another
    /// version holds nothing here.
    pub fn refusal_in(&self, op: &str, space: &str) -> Option<ExecError> {
        self.refusal_for(op, &(self.for_space)(space))
    }

    fn refusal_for(&self, op: &str, fixture: &Arc<ordo_conformance::Fixture>) -> Option<ExecError> {
        // A program applied since the pass has no hold: it was checked
        // against the tables when it was applied, and the next pass reads
        // it again.
        let down = self
            .holds
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&identity(fixture))
            .filter(|d| !d.is_empty())
            .cloned()?;
        let reads = self.reads_for(fixture);
        let first = reads.get(op)?.iter().find(|e| down.contains_key(*e))?;
        Some(ExecError::new(
            ErrorCode::ExternalChanged,
            format!(
                "op `{op}` reads the external entity `{first}`, which cannot be read now. {}",
                down[first]
            ),
        ))
    }
}

/// Run [`ExternalGate::watch_once`] on a timer, until the process ends.
pub fn spawn_watch(
    gate: Arc<ExternalGate>,
    pool: Pool,
    every: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut held = false;
        loop {
            tick.tick().await;
            match gate.watch_once(&pool).await {
                Ok(lines) => {
                    for l in &lines {
                        eprintln!("external: {l}");
                    }
                    if held && lines.is_empty() {
                        println!("external: every external table is as the program says again");
                    }
                    held = !lines.is_empty();
                }
                // A database that cannot be asked changes nothing: the last
                // answer holds until it can.
                Err(e) => {
                    eprintln!("external: the tables were not read ({e}); the hold is unchanged")
                }
            }
        }
    })
}

/// The decorator that applies an [`ExternalGate`]: an op that reads a held
/// entity is refused before it runs, and again when a live read re-checks.
pub struct ExternalGated {
    pub inner: Arc<dyn OpExecutor>,
    pub gate: Arc<ExternalGate>,
}

impl ExternalGated {
    pub fn new(inner: Arc<dyn OpExecutor>, gate: Arc<ExternalGate>) -> ExternalGated {
        ExternalGated { inner, gate }
    }
}

impl OpExecutor for ExternalGated {
    fn held(&self, op: &OpSpec, space: &str) -> Option<ExecError> {
        self.gate
            .refusal_in(&op.name, space)
            .or_else(|| self.inner.held(op, space))
    }

    fn execute<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: ordo_api::BoundParams,
        refine: Option<ordo_api::Refinement>,
        page: Option<ordo_api::PageCursor>,
        lease: Option<ordo_api::ReplayLease>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecError>> {
        self.execute_sized(op, space, bound, refine, page, lease, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_sized<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: ordo_api::BoundParams,
        refine: Option<ordo_api::Refinement>,
        page: Option<ordo_api::PageCursor>,
        lease: Option<ordo_api::ReplayLease>,
        page_rows: Option<usize>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecError>> {
        if let Some(e) = self.gate.refusal_in(&op.name, space) {
            return Box::pin(async move { Err(e) });
        }
        self.inner
            .execute_sized(op, space, bound, refine, page, lease, page_rows)
    }
}
