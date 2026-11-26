//! Grants to other Ordo deployments (`plan-federation-grants.md` §3, step G1).
//!
//! The holder's file writes what each consumer may call through its door:
//!
//! ```text
//! grant note_viewer {
//!   std.notes.get
//!   /add.*/
//!   !addAdmin
//! }
//! ```
//!
//! A line is an exact op name, a `/regex/` that must match the whole name,
//! or `*`; `!` before it takes ops away. Read top to bottom, the last line
//! that matches an op decides, and an op no line matches is not granted.
//!
//! What this module does, at every apply and every plan:
//!
//! - [`expand`] each block against the program's own ops, the derived
//!   `std.<entity>.get` and `.list` included. The door (G3) checks only the
//!   expanded list.
//! - Refuse, by name and with the fix, a pattern that matches nothing, a line
//!   that changes nothing, a pattern that is not a regex, and a
//!   `grant provider` that names something other than a sign-in connection
//!   to Ordo's identity service.
//! - Show each grant's list and what changed against the serving version
//!   ("note_viewer gains addReaction, matches /add.*/"), and the setting
//!   `ORDO_CONSUMER_<NAME>_CLIENT` that ties the name to a consumer.
//! - [`store`] the expanded lists with the version, in [`TABLE`].
//!
//! The door enforces the stored lists ([`crate::peer_door`], step G3). An
//! unset setting is a warning here: the apply goes on, and the door refuses
//! the calls that grant would cover, naming the setting.

use std::collections::{BTreeMap, BTreeSet};

use ordo_exec::{Pool, PoolError};
use ordo_syntax::{GrantDecl, GrantPattern};
use serde_json::Value;

/// Where each version's expanded grants are kept: one row per grant.
pub const TABLE: &str = "$consumergrant";

const DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$consumergrant" (
  "version"   text NOT NULL,
  "grant"     text NOT NULL,
  "provider"  boolean NOT NULL,
  "as_itself" boolean NOT NULL,
  "ops"       text NOT NULL,
  PRIMARY KEY ("version", "grant", "provider")
)"#;

/// A pattern this program's ops never match.
pub const MATCHES_NOTHING: &str = "grant_matches_nothing";
/// A line whose removal leaves the grant the same.
pub const CHANGES_NOTHING: &str = "grant_line_changes_nothing";
/// A `/…/` that is not a regex.
pub const PATTERN_INVALID: &str = "grant_pattern_invalid";
/// `grant provider <name>` where `<name>` is not a sign-in connection to
/// Ordo's identity service.
pub const PROVIDER_NOT_IDENTITY_SERVICE: &str = "grant_provider_not_identity_service";

/// The setting that ties a grant's name to the consumer's id at the
/// identity service: `ORDO_CONSUMER_<NAME>_CLIENT`, never in the file (ids
/// differ per environment; owner ruling 2026-11-14).
pub fn client_env(name: &str) -> String {
    format!("ORDO_CONSUMER_{}_CLIENT", name.to_ascii_uppercase())
}

/// One grant, expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub name: String,
    pub provider: bool,
    pub as_itself: bool,
    /// The ops granted, sorted.
    pub ops: Vec<String>,
}

impl Expanded {
    /// The header as written: `grant note_viewer`, `grant billing as
    /// itself`, `grant provider hominem`.
    pub fn header(&self) -> String {
        match (self.provider, self.as_itself) {
            (true, _) => format!("grant provider {}", self.name),
            (false, true) => format!("grant {} as itself", self.name),
            (false, false) => format!("grant {}", self.name),
        }
    }

    fn key(&self) -> (bool, String) {
        (self.provider, self.name.clone())
    }
}

/// One grant expanded, with the line that decided each op it grants, for
/// the plan's "gains X, matches /…/".
#[derive(Debug, Clone)]
pub struct Expansion {
    pub grant: Expanded,
    /// Op to the text of the line that granted it.
    pub by: BTreeMap<String, String>,
}

/// One refusal: its class and the whole line the plan prints, ending with
/// the fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub class: &'static str,
    pub text: String,
}

enum Matcher {
    All,
    Op(String),
    Regex(regex::Regex),
    /// A pattern that did not compile: it is refused, and matches nothing.
    Broken,
}

impl Matcher {
    fn matches(&self, op: &str) -> bool {
        match self {
            Matcher::All => true,
            Matcher::Op(n) => n == op,
            Matcher::Regex(r) => r.is_match(op),
            Matcher::Broken => false,
        }
    }
}

/// The largest compiled pattern accepted. A real one is a few KiB.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Which ops `lines` grant, skipping line `skip`: the last line that
/// matches an op decides.
fn decide(
    m: &[Matcher],
    negated: &[bool],
    ops: &[String],
    skip: Option<usize>,
) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for op in ops {
        let last = (0..m.len())
            .rev()
            .filter(|i| Some(*i) != skip)
            .find(|i| m[*i].matches(op));
        if let Some(i) = last {
            if !negated[i] {
                out.insert(op.clone(), i);
            }
        }
    }
    out
}

/// Expand one grant against `ops`, the program's own op names. Every
/// problem is returned, not only the first.
pub fn expand(g: &GrantDecl, ops: &[String]) -> Result<Expansion, Vec<Refusal>> {
    let header = g.header();
    let mut refusals = Vec::new();
    let at = |i: usize| format!("line {} `{}`", g.lines[i].span.line, g.lines[i].text());
    let matchers: Vec<Matcher> = g
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| match &l.pattern {
            GrantPattern::All => Matcher::All,
            GrantPattern::Op(n) => Matcher::Op(n.clone()),
            GrantPattern::Regex(r) => match regex::RegexBuilder::new(&format!("^(?:{r})$"))
                .size_limit(REGEX_SIZE_LIMIT)
                .build()
            {
                Ok(re) => Matcher::Regex(re),
                Err(e) => {
                    let why = e.to_string();
                    let why = why.lines().last().unwrap_or("").trim().to_string();
                    refusals.push(Refusal {
                        class: PATTERN_INVALID,
                        text: format!(
                            "{PATTERN_INVALID} in `{header}`, {}: not a pattern ({why}). Fix: \
                             write a Rust `regex` between the slashes, such as `/add.*/`, or \
                             name the op exactly",
                            at(i)
                        ),
                    });
                    Matcher::Broken
                }
            },
        })
        .collect();
    let negated: Vec<bool> = g.lines.iter().map(|l| l.negated).collect();
    let all = decide(&matchers, &negated, ops, None);

    for (i, m) in matchers.iter().enumerate() {
        if matches!(m, Matcher::Broken) {
            continue;
        }
        let matched: Vec<&String> = ops.iter().filter(|o| m.matches(o)).collect();
        if matched.is_empty() {
            let fix = match &g.lines[i].pattern {
                GrantPattern::Op(_) => "name an op this program declares, or remove the line",
                _ => "change the pattern to match an op this program declares, or remove the line",
            };
            refusals.push(Refusal {
                class: MATCHES_NOTHING,
                text: format!(
                    "{MATCHES_NOTHING} in `{header}`, {}: no op of this program matches it. \
                     Fix: {fix}",
                    at(i)
                ),
            });
            continue;
        }
        let without: BTreeSet<String> = decide(&matchers, &negated, ops, Some(i))
            .into_keys()
            .collect();
        let with: BTreeSet<String> = all.keys().cloned().collect();
        if without != with {
            continue;
        }
        // The line decides nothing. Overridden: a later line matches every
        // op it matches. Otherwise it repeats what the lines above (or no
        // line) already answer.
        let later = |op: &str| (i + 1..matchers.len()).find(|j| matchers[*j].matches(op));
        let overriders: BTreeSet<usize> = matched.iter().filter_map(|o| later(o)).collect();
        let text = if matched.iter().all(|o| later(o).is_some()) {
            let by: Vec<String> = overriders.iter().map(|j| at(*j)).collect();
            format!(
                "{CHANGES_NOTHING} in `{header}`, {}: every op it matches is decided by a later \
                 line ({}). Fix: remove it, or move it below the line that overrides it",
                at(i),
                by.join(", ")
            )
        } else {
            let what = if g.lines[i].negated {
                "none of the ops it matches is granted above it"
            } else {
                "every op it matches is granted without it"
            };
            format!(
                "{CHANGES_NOTHING} in `{header}`, {}: {what}. Fix: remove the line",
                at(i)
            )
        };
        refusals.push(Refusal {
            class: CHANGES_NOTHING,
            text,
        });
    }
    if !refusals.is_empty() {
        return Err(refusals);
    }
    let by = all
        .iter()
        .map(|(op, i)| (op.clone(), g.lines[*i].text()))
        .collect();
    Ok(Expansion {
        grant: Expanded {
            name: g.name.name.clone(),
            provider: g.provider,
            as_itself: g.as_itself,
            ops: all.into_keys().collect(),
        },
        by,
    })
}

/// **Who reads a table through this deployment's door**: each grant that
/// opens `std.<table>.list` or `std.<table>.get` of a native entity, by its
/// header (`grant note_viewer`, `grant provider hominem`), with the tables
/// it opens. What a plan names when a step changes a table another
/// deployment reads (`plan-evolution-federation.md` F1), and what a prune
/// lists. A pattern that is not a regex opens nothing here; the apply
/// refuses it ([`expand`]).
pub fn tables_read(schema: &ordo_schema::Schema) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for g in &schema.grants {
        let matchers: Vec<Matcher> = g
            .lines
            .iter()
            .map(|l| match &l.pattern {
                GrantPattern::All => Matcher::All,
                GrantPattern::Op(n) => Matcher::Op(n.clone()),
                GrantPattern::Regex(r) => regex::RegexBuilder::new(&format!("^(?:{r})$"))
                    .size_limit(REGEX_SIZE_LIMIT)
                    .build()
                    .map(Matcher::Regex)
                    .unwrap_or(Matcher::Broken),
            })
            .collect();
        let negated: Vec<bool> = g.lines.iter().map(|l| l.negated).collect();
        for e in schema.entities.iter().filter(|e| e.external.is_none()) {
            let ops = [
                format!("std.{}.list", e.name),
                format!("std.{}.get", e.name),
            ];
            if !decide(&matchers, &negated, &ops, None).is_empty() {
                out.entry(g.header()).or_default().insert(e.name.clone());
            }
        }
    }
    out
}

/// The grants, by header, that open `table` to another deployment
/// ([`tables_read`]).
pub fn readers_of(schema: &ordo_schema::Schema, table: &str) -> Vec<String> {
    tables_read(schema)
        .into_iter()
        .filter(|(_, t)| t.contains(table))
        .map(|(g, _)| g)
        .collect()
}

/// The program's own ops, from its compiled document: every authored op and
/// every derived `std.<entity>.*`, sorted.
pub fn program_ops(doc: &Value) -> Vec<String> {
    let mut ops: Vec<String> = doc["ops"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| o["op"].as_str().map(str::to_string))
        .collect();
    ops.sort();
    ops.dedup();
    ops
}

/// Is the sign-in connection `name` one to Ordo's identity service? It is
/// when it is an OIDC connection (`ORDO_AUTH_<NAME>_KIND=oidc`) whose issuer
/// is the identity service's origin (`ORDO_IDENTITY_ORIGIN`). A preset
/// (Google, Microsoft) never is. `Err` says why not, with the fix.
pub fn provider_is_identity_service(
    name: &str,
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    let p = crate::federation::prefix(name);
    let get = |k: &str| {
        var(&format!("{p}{k}"))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let origin_env = crate::identity_keys::ISSUER_ENV;
    let trim = |s: &str| s.trim().trim_end_matches('/').to_string();
    if let Some(preset) = get("PRESET") {
        return Err(format!(
            "`{name}` is the `{preset}` preset ({p}PRESET), an outside provider: everything \
             registered there is anyone. Fix: name the connection to Ordo's identity service, \
             or write a grant for each consumer: `grant <name> {{ … }}`"
        ));
    }
    match get("KIND").as_deref() {
        Some("oidc") => {}
        Some(other) => {
            return Err(format!(
                "`{name}` is a `{other}` connection ({p}KIND), not Ordo's identity service. \
                 Fix: name the OIDC connection whose {p}ISSUER is the identity service"
            ))
        }
        None => {
            return Err(format!(
                "`{name}` is not a sign-in connection of this deployment: {p}KIND is unset. \
                 Fix: set {p}KIND=oidc and {p}ISSUER to Ordo's identity service, or name the \
                 connection that is"
            ))
        }
    }
    let Some(origin) = var(origin_env).map(|v| trim(&v)).filter(|v| !v.is_empty()) else {
        return Err(format!(
            "`{name}` cannot be told to be Ordo's identity service: {origin_env} is unset. \
             Fix: set {origin_env} (and {}) to the identity service this deployment uses",
            crate::identity_keys::URL_ENV
        ));
    };
    match get("ISSUER").map(|i| trim(&i)) {
        Some(issuer) if issuer == origin => Ok(()),
        Some(issuer) => Err(format!(
            "`{name}` is an outside OIDC provider: its issuer `{issuer}` ({p}ISSUER) is not \
             Ordo's identity service `{origin}` ({origin_env}), and everything registered at an \
             outside provider is anyone. Fix: name the connection to the identity service, or \
             write a grant for each consumer: `grant <name> {{ … }}`"
        )),
        None => Err(format!(
            "`{name}` has no issuer: {p}ISSUER is unset. Fix: set it to Ordo's identity \
             service, `{origin}`"
        )),
    }
}

/// What the plan and the apply learned about the program's grants.
#[derive(Debug, Clone, Default)]
pub struct GrantReport {
    pub expanded: Vec<Expanded>,
    /// Every refusal, one line each, ending with the fix.
    pub refusals: Vec<Refusal>,
    /// What the plan shows: each grant's list, its changes against the
    /// serving version, and its setting.
    pub notes: Vec<String>,
}

impl GrantReport {
    /// The refusal lines, for [`crate::apply_op::ApplyRefused::Grant`].
    pub fn refusal_lines(&self) -> Vec<String> {
        self.refusals.iter().map(|r| r.text.clone()).collect()
    }
}

/// Expand every grant of `schema` against `ops`, check each `grant
/// provider`, and write the plan's lines against `serving`, the expanded
/// grants of the version serving now. `var` reads a setting.
pub fn check(
    grants: &[GrantDecl],
    ops: &[String],
    serving: &[Expanded],
    var: &dyn Fn(&str) -> Option<String>,
) -> GrantReport {
    let mut r = GrantReport::default();
    let before: BTreeMap<(bool, String), &Expanded> =
        serving.iter().map(|e| (e.key(), e)).collect();
    let mut seen: BTreeSet<(bool, String)> = BTreeSet::new();
    for g in grants {
        if g.provider {
            if let Err(why) = provider_is_identity_service(&g.name.name, var) {
                r.refusals.push(Refusal {
                    class: PROVIDER_NOT_IDENTITY_SERVICE,
                    text: format!("{PROVIDER_NOT_IDENTITY_SERVICE} in `{}`: {why}", g.header()),
                });
            }
        }
        let x = match expand(g, ops) {
            Ok(x) => x,
            Err(rs) => {
                r.refusals.extend(rs);
                continue;
            }
        };
        let e = &x.grant;
        seen.insert(e.key());
        // A provider grant is labelled as such: `grant ids` and `grant
        // provider ids` are 2 grants.
        let label = if e.provider {
            format!("provider {}", e.name)
        } else {
            e.name.clone()
        };
        let who = if e.provider {
            "every deployment registered at this identity service, on behalf of a person"
        } else if e.as_itself {
            "on behalf of a person or as itself"
        } else {
            "on behalf of a person"
        };
        r.notes.push(format!(
            "grant       {label}: {} op(s), {who}: {}",
            e.ops.len(),
            e.ops.join(", ")
        ));
        match before.get(&e.key()) {
            None => r.notes.push(format!("grant       {label}: new")),
            Some(b) => {
                let old: BTreeSet<&String> = b.ops.iter().collect();
                let new: BTreeSet<&String> = e.ops.iter().collect();
                for op in new.difference(&old) {
                    let line = x.by.get(*op).cloned().unwrap_or_default();
                    let how = if line == **op {
                        "named".to_string()
                    } else {
                        format!("matches {line}")
                    };
                    r.notes
                        .push(format!("grant       {label} gains {op}, {how}"));
                }
                for op in old.difference(&new) {
                    r.notes.push(format!("grant       {label} loses {op}"));
                }
                if b.as_itself != e.as_itself {
                    r.notes.push(format!(
                        "grant       {label}: {}",
                        if e.as_itself {
                            "now allows calls as itself"
                        } else {
                            "no longer allows calls as itself"
                        }
                    ));
                }
            }
        }
        if !e.provider {
            let env = client_env(&e.name);
            match var(&env)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            {
                Some(id) => r.notes.push(format!("grant       {}: {env}={id}", e.name)),
                None => r.notes.push(format!(
                    "warning     grant {}: {env} is not set, so no consumer is tied to it. Set it \
                     to the consumer's client id at the identity service. Until then the door \
                     refuses every call that grant would cover",
                    e.name
                )),
            }
        } else if let Some(issuer) = var(&format!("{}ISSUER", crate::federation::prefix(&e.name)))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            // Who it reaches: any deployment that service vouches for, and
            // only with a person (step G6).
            r.notes.push(format!(
                "grant       {label}: any deployment registered at {issuer}, on behalf of a \
                 person; a call as itself needs a grant of its own"
            ));
        }
        r.expanded.push(x.grant);
    }
    for b in serving {
        if !seen.contains(&b.key()) && r.refusals.is_empty() {
            r.notes.push(format!(
                "grant       {}: removed; it granted {}",
                b.header(),
                b.ops.join(", ")
            ));
        }
    }
    r
}

async fn ensure(pool: &Pool) -> Result<(), PoolError> {
    pool.rows(DDL).await.map(|_| ())
}

/// The expanded grants stored with `version`. None when nothing is stored.
pub async fn stored(pool: &Pool, version: &str) -> Result<Vec<Expanded>, PoolError> {
    let sql = format!(
        "SELECT \"grant\", \"provider\"::text, \"as_itself\"::text, \"ops\" FROM \"{TABLE}\" \
         WHERE \"version\" = {} ORDER BY \"provider\", \"grant\"",
        ordo_exec::sql_lit(version)
    );
    let rows = match pool.rows(&sql).await {
        Ok(rows) => rows,
        Err(e) if e.is_missing_table() => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let mut it = r.into_iter();
            let name = it.next()??;
            let provider = it.next()?? == "true";
            let as_itself = it.next()?? == "true";
            let ops: Vec<String> = serde_json::from_str(&it.next()??).ok()?;
            Some(Expanded {
                name,
                provider,
                as_itself,
                ops,
            })
        })
        .collect())
}

/// The expanded grants of the version serving now. None when nothing
/// serves or it stored none.
pub async fn serving(pool: &Pool) -> Result<Vec<Expanded>, String> {
    let active = crate::versions::Versions::on(pool)
        .active_version()
        .await
        .ok()
        .flatten();
    match active {
        Some(v) => stored(pool, &v).await.map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

/// Store `grants` as `version`'s, replacing what was stored for it.
pub async fn store(pool: &Pool, version: &str, grants: &[Expanded]) -> Result<(), PoolError> {
    if grants.is_empty() && !has_table(pool).await? {
        return Ok(());
    }
    ensure(pool).await?;
    let v = ordo_exec::sql_lit(version);
    // One string: the statements run as one implicit transaction, so a
    // reader never sees the version with half its grants.
    let mut sql = format!("DELETE FROM \"{TABLE}\" WHERE \"version\" = {v};");
    for g in grants {
        let ops = serde_json::to_string(&g.ops).unwrap_or_else(|_| "[]".into());
        sql.push_str(&format!(
            " INSERT INTO \"{TABLE}\" (\"version\", \"grant\", \"provider\", \"as_itself\", \"ops\") \
             VALUES ({v}, {}, {}, {}, {});",
            ordo_exec::sql_lit(&g.name),
            g.provider,
            g.as_itself,
            ordo_exec::sql_lit(&ops)
        ));
    }
    pool.rows(&sql).await.map(|_| ())
}

async fn has_table(pool: &Pool) -> Result<bool, PoolError> {
    match pool
        .rows(&format!("SELECT 1 FROM \"{TABLE}\" LIMIT 1"))
        .await
    {
        Ok(_) => Ok(true),
        Err(e) if e.is_missing_table() => Ok(false),
        Err(e) => Err(e),
    }
}

/// The program's grants, expanded and checked against the version serving
/// on `pool`, with the process environment's settings. What the plan and
/// the apply both call, so a plan refuses exactly what an apply would.
pub async fn plan(
    pool: &Pool,
    schema: &ordo_schema::Schema,
    doc: &Value,
) -> Result<GrantReport, String> {
    plan_with(pool, schema, doc, &|k| std::env::var(k).ok()).await
}

/// [`plan`], reading each setting through `var`.
pub async fn plan_with(
    pool: &Pool,
    schema: &ordo_schema::Schema,
    doc: &Value,
    var: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<GrantReport, String> {
    let serving = serving(pool).await?;
    let ops = program_ops(doc);
    let mut report = check(&schema.grants, &ops, &serving, var);
    // **The consumers this change would break** (`plan-federation-grants.md`
    // §4, step G4): each op a consumer recorded it calls that these grants
    // no longer open to it, named before the apply. Warned, not refused.
    if report.refusals.is_empty() {
        let uses = crate::peer_ops::recorded(pool)
            .await
            .map_err(|e| format!("the consumers' recorded uses could not be read: {e}"))?;
        report.notes.extend(crate::peer_ops::breaks(
            &report.expanded,
            &serving,
            &ops,
            &uses,
            var,
        ));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants(src: &str) -> Vec<GrantDecl> {
        ordo_syntax::parse_program(src).expect("parses").grants
    }

    fn ops(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    const OPS: &[&str] = &[
        "addAdmin",
        "addNote",
        "addReaction",
        "archiveNote",
        "std.notes.get",
        "std.notes.list",
        "std.notesAccess.get",
    ];

    #[test]
    fn last_match_wins_and_no_match_is_not_granted() {
        let g = grants("grant v {\n  /add.*/\n  !addAdmin\n  std.notes.get\n}\n");
        let x = expand(&g[0], &ops(OPS)).expect("expands");
        assert_eq!(x.grant.ops, ["addNote", "addReaction", "std.notes.get"]);
        assert_eq!(x.by["addNote"], "/add.*/");

        // Bottom overrides top: put back by a later line.
        let g = grants("grant v {\n  *\n  !/add.*/\n  addNote\n}\n");
        let x = expand(&g[0], &ops(OPS)).expect("expands");
        assert_eq!(
            x.grant.ops,
            [
                "addNote",
                "archiveNote",
                "std.notes.get",
                "std.notes.list",
                "std.notesAccess.get"
            ]
        );
        assert_eq!(x.by["archiveNote"], "*");
    }

    #[test]
    fn every_problem_is_named() {
        let g = grants("grant v {\n  /edit.*/\n  addNote\n  /add.*/\n  /(/\n  !archiveX\n}\n");
        let e = expand(&g[0], &ops(OPS)).expect_err("refused");
        let classes: Vec<&str> = e.iter().map(|r| r.class).collect();
        assert_eq!(
            classes,
            [
                PATTERN_INVALID,
                MATCHES_NOTHING,
                CHANGES_NOTHING,
                MATCHES_NOTHING
            ],
            "{e:#?}"
        );
        assert!(e[2].text.contains("line 3 `addNote`"), "{}", e[2].text);
        assert!(e[2].text.contains("line 4 `/add.*/`"), "{}", e[2].text);
    }

    #[test]
    fn a_negation_with_nothing_to_take_away_changes_nothing() {
        let g = grants("grant v {\n  !archiveNote\n  addNote\n}\n");
        let e = expand(&g[0], &ops(OPS)).expect_err("refused");
        assert_eq!(e[0].class, CHANGES_NOTHING);
        assert!(e[0].text.contains("none of the ops"), "{}", e[0].text);
    }

    #[test]
    fn a_provider_must_be_the_identity_service() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let ok = env(&[
            ("ORDO_AUTH_HOMINEM_KIND", "oidc"),
            ("ORDO_AUTH_HOMINEM_ISSUER", "https://auth.example.com/"),
            ("ORDO_IDENTITY_ORIGIN", "https://auth.example.com"),
        ]);
        assert!(provider_is_identity_service("hominem", &ok).is_ok());
        let google = env(&[("ORDO_AUTH_G_PRESET", "google")]);
        assert!(provider_is_identity_service("g", &google)
            .unwrap_err()
            .contains("outside provider"));
        let outside = env(&[
            ("ORDO_AUTH_CORP_KIND", "oidc"),
            ("ORDO_AUTH_CORP_ISSUER", "https://sso.corp.example"),
            ("ORDO_IDENTITY_ORIGIN", "https://auth.example.com"),
        ]);
        assert!(provider_is_identity_service("corp", &outside)
            .unwrap_err()
            .contains("outside OIDC provider"));
        let none = env(&[]);
        assert!(provider_is_identity_service("x", &none)
            .unwrap_err()
            .contains("ORDO_AUTH_X_KIND is unset"));
    }
}
