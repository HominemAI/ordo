//! **Pre-checks on every tenant** (`plan-versions.md` §2.3, increment 9;
//! ruled 2026-10-01, `forward-gaps` asks 12 and 13; 27w).
//!
//! A stored plan is the same in every environment. The data it meets is
//! not: one tenant holds duplicates under a new `unique`, another a value
//! over a new bound. So before a step runs, every database the deployment
//! serves from is asked the questions the step's changes raise, and every
//! issue is reported at once, each with the fix in Ordo's terms. One
//! failure never stops the rest.
//!
//! # What runs, per database
//!
//! - **The data probes** of `schema_apply::probe`, through
//!   [`sa::plan_with`]: duplicates under a new unique, nulls under a new
//!   required field, values outside a new set or bound, refs with no
//!   target. A shared database answers for every space in it, so the lines
//!   name each space. A database per tenant answers for its own.
//! - **The external check** (`external_check`): a foreign table the version
//!   reads, as it is in that database now.
//!
//! A database that records another version than the step's from-version is
//! not probed: the step does not run there, and its own step runs these
//! checks when its stored path reaches it (`run_step`). It is listed, so a
//! clean answer never hides a database nobody asked.
//!
//! # Once per deployment
//!
//! - **The readers** of each changed table (F1), from the render.
//! - **The sign-in method removal check** (ruled 2026-11-23, Q6): a version
//!   whose `auth` block drops a method counts the people left with no
//!   method. `ordo evolve` stops on it, and goes on only with the deployment
//!   owner's approval, which names the count and is recorded on the stream.
//!   The warning gives the migration steps.
//! - **The `requires` check** is not run: the `requires` declaration is not
//!   in the language yet (`plan-configuration-in-the-file.md` §3). The
//!   answer says so rather than reporting a pass.
//!
//! Everything here reads. Nothing is written.

use std::collections::BTreeSet;

use ordo_exec::route::PoolResolver;
use ordo_exec::Pool;
use ordo_schema::diff::Change;
use serde_json::{json, Value};

use crate::evolve::{EvolveRefusal, StoredPlan};
use crate::schema_apply::{self as sa, Probe, Program};

/// What the answer says about the `requires` check until it exists.
pub const REQUIRES_NOT_BUILT: &str =
    "the `requires` check did not run: the `requires` declaration is not in the language yet";

/// One thing a database holds that the step's new shape would refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The database: `default`, or the target's label.
    pub database: String,
    /// `data`, `external` or `unreachable`.
    pub kind: &'static str,
    /// The taxonomy code of the change (`C1`), or the external's class.
    pub code: String,
    /// The change, or the external, in words.
    pub change: String,
    /// What was asked and found.
    pub what: String,
    /// The offending values, one line each, naming the space.
    pub lines: Vec<String>,
    /// What the developer does about it.
    pub fix: String,
}

impl Issue {
    pub fn to_json(&self) -> Value {
        json!({
            "database": self.database, "kind": self.kind, "code": self.code,
            "change": self.change, "what": self.what, "lines": self.lines,
            "fix": self.fix,
        })
    }

    /// The issue as a person reads it, on one line.
    pub fn line(&self) -> String {
        let mut s = format!("database `{}`: {} {}", self.database, self.code, self.what);
        if !self.lines.is_empty() {
            s.push_str(&format!(" ({})", self.lines.join("; ")));
        }
        s.push_str(&format!(". Fix: {}", self.fix));
        s
    }
}

/// What a data issue's fix is, in Ordo's terms. Ordo never picks which row
/// survives, so every fix is the developer's act on their own data, or a
/// change to the version.
pub fn data_fix(c: &Change, to: &str) -> String {
    match c {
        Change::AddUnique { entity, field, .. } => format!(
            "make `{entity}`.`{field}` distinct in each space listed, through the app's own \
             ops, or leave `unique` out of `{to}`. Ordo does not choose which row survives"
        ),
        Change::AddCompositeUnique { entity, key, .. } => format!(
            "make `{entity}` ({}) distinct in each space listed, through the app's own ops, \
             or leave the `unique` out of `{to}`",
            key.join(", ")
        ),
        Change::AddEnum {
            entity,
            field,
            values,
            ..
        } => format!(
            "map each listed value of `{entity}`.`{field}` to one of {}, or add it to the set \
             in `{to}`",
            values.join(", ")
        ),
        Change::AddForeignKey {
            entity,
            field,
            target,
            ..
        } => format!(
            "point the listed `{entity}` rows' `{field}` at a `{target}` that exists in the \
             same space, or clear it if the field is optional"
        ),
        Change::AddCheck { entity, source, .. } => format!(
            "change the listed `{entity}` rows so `check {source}` holds, or loosen the check \
             in `{to}`"
        ),
        Change::RequireField { entity, field } | Change::AddRequiredField { entity, field } => {
            format!(
                "give every listed `{entity}` row a `{field}`, or keep `{field}` optional in \
                 `{to}`"
            )
        }
        Change::Normalize { entity, field, .. } => format!(
            "merge or rename the colliding `{entity}`.`{field}` values in each space listed"
        ),
        Change::ChangePlaces { entity, field, .. } => format!(
            "correct the listed `{entity}`.`{field}` amounts, or state the other choice: \
             `--places {entity}.{field}=rescale` or `--places {entity}.{field}=keep`"
        ),
        _ => "correct the listed rows, or render the change in two versions".into(),
    }
}

/// The issues one database's plan holds: each change a probe found data
/// against or could not ask, and each external it cannot read. Orphaned
/// changes and the deploy-order check (`drop_read_by_active_op`) are not
/// issues: a step orphans instead of dropping.
pub fn issues_of(database: &str, plan: &sa::Plan, to: &str) -> Vec<Issue> {
    let mut out = Vec::new();
    for a in &plan.assessed {
        if matches!(
            a.change,
            Change::DropField { .. } | Change::DropEntity { .. }
        ) {
            continue;
        }
        let code = a.change.code().to_string();
        let change = a.change.to_string();
        match &a.probe {
            Some(Ok(Probe::Dirty { what, lines })) => out.push(Issue {
                database: database.to_string(),
                kind: "data",
                code,
                change,
                what: what.clone(),
                lines: lines.clone(),
                fix: data_fix(&a.change, to),
            }),
            Some(Err(why)) => out.push(Issue {
                database: database.to_string(),
                kind: "data",
                code,
                change,
                what: format!("the database could not be asked: {why}"),
                lines: Vec::new(),
                fix: "bring the database back and run the step's `ordo evolve` again".into(),
            }),
            _ => {
                // A refusal the probe did not raise: a choice the change
                // needs, refused by the places check on this database.
                for r in a.refused.iter().filter(|r| {
                    *r != "drop_read_by_active_op"
                        && *r != "probe_not_empty"
                        && !r.starts_with("probe_unavailable")
                }) {
                    out.push(Issue {
                        database: database.to_string(),
                        kind: "data",
                        code: code.clone(),
                        change: change.clone(),
                        what: r.clone(),
                        lines: Vec::new(),
                        fix: data_fix(&a.change, to),
                    });
                }
            }
        }
    }
    for p in &plan.external.problems {
        out.push(Issue {
            database: database.to_string(),
            kind: "external",
            code: p.kind.class().to_string(),
            change: format!("external `{}`", p.entity),
            what: p.detail.clone(),
            lines: Vec::new(),
            fix: "make the foreign table match the version's translation in this database, \
                  or change the translation in the version"
                .into(),
        });
    }
    out
}

/// A database the pre-check did not probe, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotProbed {
    pub database: String,
    pub why: String,
}

/// The sign-in method removal check's answer (ruled 2026-11-23, Q6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInRemoval {
    pub from: String,
    pub to: String,
    /// The methods the from-version serves and the to-version does not.
    pub removed: Vec<String>,
    /// The methods the to-version keeps.
    pub kept: Vec<String>,
    /// The people whose every live method is a removed one.
    pub people: u64,
    /// The peers either version reads (`peer <name>` lines). A peer matches
    /// people by their sign-in, so a removed method ends the match there
    /// for those who used it (`plan-evolution-federation.md` §5).
    pub peers: Vec<String>,
}

impl SignInRemoval {
    /// The migration steps the warning gives.
    pub fn steps(&self) -> Vec<String> {
        let removed = self
            .removed
            .iter()
            .map(|m| format!("`{m}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let kept = if self.kept.is_empty() {
            "none: the new version serves no method".to_string()
        } else {
            self.kept
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut steps = vec![
            format!(
                "ask those {} person(s) to add a method `{}` keeps ({kept}) before it is active",
                self.people, self.to
            ),
            format!(
                "or keep {removed} in the `auth` block one more version, and tell them it is \
                 going"
            ),
            format!(
                "to go on anyway: `ordo evolve --from {} --to {} --approve-sign-in-removal {}`. \
                 The approval is recorded on the deployment's stream",
                self.from, self.to, self.people
            ),
        ];
        steps.extend(self.peer_notes());
        steps
    }

    /// One note per peer. Whether the removed method is the only one this
    /// deployment shares with that peer is the peer's `auth` block, which
    /// this deployment cannot read, so each note says what follows and asks
    /// the peer's owner to check.
    pub fn peer_notes(&self) -> Vec<String> {
        let removed = self
            .removed
            .iter()
            .map(|m| format!("`{m}`"))
            .collect::<Vec<_>>()
            .join(", ");
        self.peers
            .iter()
            .map(|peer| {
                format!(
                    "this version removes {removed}. Peer `{peer}` matches people by their \
                     sign-in, so people who used it stop matching there. They read nothing in \
                     `{peer}`, and nothing leaks. If it is the only method shared with \
                     `{peer}`, nobody matches until they sign in another way. Tell `{peer}`'s \
                     owner"
                )
            })
            .collect()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "removed": self.removed, "kept": self.kept, "people": self.people,
            "steps": self.steps(), "peers": self.peers, "peerNotes": self.peer_notes(),
        })
    }
}

/// The methods an `auth` block serves. `None`: the version has no block,
/// so the instance's settings decide and the file says nothing.
fn methods(p: &Program) -> Option<Vec<String>> {
    p.schema
        .auth
        .as_ref()
        .map(|a| ordo_schema::auth::AuthValues::resolve(a).methods)
}

/// The method an authenticator id signs in with: `passkey:…` is `passkey`,
/// `oidc:google:…` and `oauth2:github:…` are the provider's name.
const METHOD_OF: &str = "CASE WHEN split_part(a.\"authenticator\", ':', 1) IN ('oidc', 'oauth2') \
     THEN split_part(a.\"authenticator\", ':', 2) \
     ELSE split_part(a.\"authenticator\", ':', 1) END";

/// A live authenticator of a live person. `to_jsonb` reads a column a
/// table made before recovery or deletion does not have as absent.
const LIVE: &str = "COALESCE(to_jsonb(a) ->> 'retired_case', '') = '' \
     AND COALESCE((to_jsonb(p) ->> 'deleted_at')::bigint, 0) = 0";

/// **The sign-in method removal check.** `None` when the step removes no
/// method. Reads the deployment's sign-in tables in `registry`; a
/// deployment where nobody has signed up counts nobody.
pub async fn sign_in_removal(
    registry: &Pool,
    x: &Program,
    y: &Program,
    from: &str,
    to: &str,
) -> Result<Option<SignInRemoval>, EvolveRefusal> {
    let Some(kept) = methods(y) else {
        return Ok(None);
    };
    let in_use = |sql: String| async move {
        match registry.rows(&sql).await {
            Ok(rows) => Ok(rows),
            Err(e) if e.is_missing_table() => Ok(Vec::new()),
            Err(e) => Err(EvolveRefusal::Store(format!(
                "reading the sign-in tables for the removal check: {e}"
            ))),
        }
    };
    // What X serves: its block, or with none, every method in use.
    let served: Vec<String> = match methods(x) {
        Some(m) => m,
        None => in_use(format!(
            "SELECT DISTINCT {METHOD_OF} FROM \"$authenticator\" a \
             LEFT JOIN \"$principal\" p ON p.\"id\" = a.\"principal\" WHERE {LIVE} ORDER BY 1"
        ))
        .await?
        .into_iter()
        .filter_map(|r| r.first().cloned().flatten())
        .collect(),
    };
    let kept_set: BTreeSet<&str> = kept.iter().map(String::as_str).collect();
    let removed: Vec<String> = served
        .into_iter()
        .filter(|m| !kept_set.contains(m.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if removed.is_empty() {
        return Ok(None);
    }
    let list = |ms: &[String]| {
        if ms.is_empty() {
            "NULL".to_string()
        } else {
            ms.iter()
                .map(|m| ordo_exec::sql_lit(m))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    // A person counts when a live method of theirs is removed and none of
    // their live methods is kept.
    let rows = in_use(format!(
        "SELECT count(*) FROM ( \
           SELECT a.\"principal\" FROM \"$authenticator\" a \
           LEFT JOIN \"$principal\" p ON p.\"id\" = a.\"principal\" \
           WHERE {LIVE} \
           GROUP BY a.\"principal\" \
           HAVING bool_or({METHOD_OF} IN ({})) AND NOT bool_or({METHOD_OF} IN ({})) \
         ) stranded",
        list(&removed),
        list(&kept),
    ))
    .await?;
    let people = rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    Ok(Some(SignInRemoval {
        from: from.to_string(),
        to: to.to_string(),
        removed,
        kept,
        people,
        peers: x
            .schema
            .peers
            .iter()
            .chain(&y.schema.peers)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }))
}

/// Everything the pre-checks found for one stored step.
#[derive(Debug, Clone, Default)]
pub struct PreChecks {
    /// The databases probed, by label.
    pub checked: Vec<String>,
    /// The databases not probed, each with why.
    pub not_probed: Vec<NotProbed>,
    /// Every issue, by database, then in change order.
    pub issues: Vec<Issue>,
    /// Checks that did not run, each with why.
    pub not_run: Vec<String>,
}

impl PreChecks {
    pub fn to_json(&self) -> Value {
        json!({
            "checked": self.checked,
            "notProbed": self.not_probed.iter().map(|n| json!({
                "database": n.database, "why": n.why,
            })).collect::<Vec<_>>(),
            "issues": self.issues.iter().map(Issue::to_json).collect::<Vec<_>>(),
            "notRun": self.not_run,
            "ok": self.issues.is_empty(),
        })
    }
}

/// What one database answered.
enum Answer {
    Checked(Vec<Issue>),
    NotProbed(String),
}

async fn check_one(
    label: String,
    pool: Result<Pool, String>,
    x: &Program,
    y: &Program,
    options: &sa::Choices,
    plan: &StoredPlan,
) -> (String, Answer) {
    let pool = match pool {
        Ok(p) => p,
        Err(why) => {
            let issue = Issue {
                database: label.clone(),
                kind: "unreachable",
                code: "unreachable".into(),
                change: "the database".into(),
                what: format!("could not be reached: {why}"),
                lines: Vec::new(),
                fix: "bring the database back and run the step's `ordo evolve` again".into(),
            };
            return (label, Answer::Checked(vec![issue]));
        }
    };
    let recorded = match ordo_exec::spacedb::recorded_version(&pool).await {
        Ok(ordo_exec::spacedb::TargetVersion::At(v)) => Some(v),
        Ok(ordo_exec::spacedb::TargetVersion::Unknown) => None,
        Err(e) => {
            let issue = Issue {
                database: label.clone(),
                kind: "unreachable",
                code: "unreachable".into(),
                change: "the database".into(),
                what: format!("its recorded version could not be read: {e}"),
                lines: Vec::new(),
                fix: "bring the database back and run the step's `ordo evolve` again".into(),
            };
            return (label, Answer::Checked(vec![issue]));
        }
    };
    match recorded.as_deref() {
        Some(v) if v == plan.from => {}
        Some(v) if v == plan.to => {
            return (
                label,
                Answer::NotProbed(format!("it already records `{}`", plan.to)),
            )
        }
        Some(v) => {
            return (
                label,
                Answer::NotProbed(format!(
                    "it records `{v}`; its stored path runs these checks when it reaches \
                     `{}`",
                    plan.from
                )),
            )
        }
        None => {
            return (
                label,
                Answer::NotProbed("it records no version: nothing is applied there yet".into()),
            )
        }
    }
    match sa::plan_with(&pool, y, options, Some(&x.schema)).await {
        Ok(mut p) => {
            for a in &mut p.assessed {
                crate::evolve::release_held(a, &options.held);
            }
            let issues = issues_of(&label, &p, &plan.to);
            (label, Answer::Checked(issues))
        }
        Err(why) => {
            let issue = Issue {
                database: label.clone(),
                kind: "unreachable",
                code: "unreachable".into(),
                change: "the database".into(),
                what: format!("could not be planned: {why}"),
                lines: Vec::new(),
                fix: "bring the database back and run the step's `ordo evolve` again".into(),
            };
            (label, Answer::Checked(vec![issue]))
        }
    }
}

/// **Run the pre-checks of a stored step on every database** the resolver
/// routes to. Every database is asked, whatever an earlier one answered.
/// Reads; changes nothing.
pub async fn precheck(
    registry: &Pool,
    resolver: &dyn PoolResolver,
    plan: &StoredPlan,
) -> Result<PreChecks, EvolveRefusal> {
    use futures::StreamExt;
    let v = crate::versions::Versions::on(registry);
    let (Some(a), Some(b)) = (v.get(&plan.from).await?, v.get(&plan.to).await?) else {
        return Err(crate::versions::VersionRefusal::UnknownVersion(plan.from.clone()).into());
    };
    let x = crate::evolve::compiled(&a)?;
    let y = crate::evolve::compiled(&b)?;
    let mut options = crate::evolve::parse_options(&plan.options)?;
    // What the step holds to its close is checked against the copied rows,
    // not probed here (11b part 4).
    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).unwrap_or_default();
    options.held = crate::evolve::held_of(&receipt)
        .into_iter()
        .map(|h| h.name)
        .collect();
    let targets = resolver
        .targets()
        .await
        .map_err(|e| EvolveRefusal::Store(format!("could not list the databases: {e}")))?;
    let mut answers: Vec<(String, Answer)> = futures::stream::iter(targets)
        .map(|t| check_one(t.label, t.pool, &x, &y, &options, plan))
        .buffer_unordered(resolver.walk_concurrency().max(1))
        .collect()
        .await;
    // `default` first, then by label: the same order every time.
    answers.sort_by(|(a, _), (b, _)| {
        (a != ordo_exec::route::DEFAULT_LABEL, a).cmp(&(b != ordo_exec::route::DEFAULT_LABEL, b))
    });
    let mut out = PreChecks {
        not_run: vec![REQUIRES_NOT_BUILT.to_string()],
        ..PreChecks::default()
    };
    for (label, answer) in answers {
        match answer {
            Answer::Checked(issues) => {
                out.checked.push(label);
                out.issues.extend(issues);
            }
            Answer::NotProbed(why) => out.not_probed.push(NotProbed {
                database: label,
                why,
            }),
        }
    }
    Ok(out)
}
