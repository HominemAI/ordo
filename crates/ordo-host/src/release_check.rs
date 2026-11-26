//! **The release check** (`plan-versions.md` §2.6 and §2.8, increment 6).
//!
//! Every program version this deployment may serve must still compile and
//! load under a new Ordo release. Only the new binary can say so, and the
//! running host is the old one. So the check is a mode of the new image
//! (ruled 2026-11-23, Q8): `ordo release-check --direct`, run by the
//! deployment owner before the image tag changes. It reads the control
//! database, serves nothing, changes no pin, and writes one
//! `$release_checks` row. It is a `--direct` use and is recorded as one.
//!
//! The same check runs at the first boot of each release ([`at_boot`]).
//! There it only logs and records. A version that fails closes its own
//! tenants and every other tenant serves: an upgrade never stops a running
//! deployment (owner, 2026-11-06).
//!
//! **The checked set** ([`checked_set`]):
//!
//! - every version a tenant is pinned to, and the active version, which a
//!   tenant with no pin yet is served (a database from before pins);
//! - `versions.new_tenant` and every name in `versions.served`;
//! - every open move's target, every tenant's own move target, every
//!   revoke's `--move-to`;
//! - every version on a stored path between any 2 of those (§2.3).
//!
//! **What is checked, per version** ([`check_version`]):
//!
//! 1. `identity`: the stored source fingerprint is still what its row
//!    records (the digest at load, §1.4 step 4).
//! 2. `compile`: the stored source compiles under this release.
//! 3. `auth`: its `auth` block is inside this release's platform ranges.
//!    `ordo-schema` checks the ranges as it compiles; a refusal it names
//!    `auth_duration_range` or `invalid_auth` is reported under this check.
//! 4. `specs`: the door registers every op, and none takes a name this
//!    release reserves for the platform.
//! 5. `shape`: the load gate over an empty shadow of the stored DDL.
//! 6. `functions`: each function's code is stored under the hash its
//!    version names, and the version's function table builds.
//! 7. `plan`: each stored plan on its paths re-renders to its digest.
//!
//! The release moving a version is reported as a note, not a failure: a
//! version whose stored build differs from what this release compiles is a
//! new build of the same version (§1.1).
//!
//! **Read-only.** The check reads with plain queries that tolerate a
//! control database from any earlier release: a missing table or column
//! reads as empty. It never runs the registry's own setup. Two things are
//! written: one `$release_checks` row, and the `$direct_uses` row the CLI
//! writes before it acts. The shape gate and the plan re-render each build
//! a shadow namespace and drop it, as a load and `ordo evolve` do; no
//! stored row changes.
//!
//! **The override** ([`record_override`]). `ordo release-check` exits
//! non-zero when a version fails, so a deploy pipeline stops before the
//! tag changes. `--override --reason <why>` records the deployment owner's
//! decision to upgrade anyway, naming the versions and tenants that will
//! close, and exits 0. A security upgrade is never blocked; only the named
//! tenants close.
//!
//! **Editions** (increment 6b, `ordo_plan::edition`): a version whose
//! language edition is outside `editions.load_window` fails under the
//! `edition` check, naming `ordo fix`. One inside the window is a note,
//! marked `soon` when the window ends within 3 months of this release.
//!
//! **Peers and readers** ([`neighbours`], `plan-evolution-federation.md`
//! §5, owner ruling 2026-11-23, Q16). Each peer a checked version names is
//! listed with the Ordo release it last started under (its `$ordorelease`,
//! granted to its readers), and each reader this deployment grants to is
//! listed by its role and, when it has granted it (owner ruling 2026-11-25,
//! `ordo_exec::peer::grant_release_to_peers`: the `release` column of its
//! `$ordorelease`, to the role that owns this deployment's schema), the
//! Ordo release it runs. Neither fails the check: the sign-in
//! columns a reader reads are a contract (`ordo_exec::sign_in_contract`),
//! kept for `editions.load_window` after a release changes one. A column
//! this release retires is named with the date it stops being granted.
//!
//! Function code is checked as the host loads it, by hash and table; the
//! JavaScript is compiled by the engine at each call.

use std::collections::{BTreeMap, BTreeSet};

use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError};
use serde_json::{json, Value};

use crate::versions::Artifact;

/// The table, in the control database.
const DDL: &str = "CREATE TABLE IF NOT EXISTS \"$release_checks\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"release\" text NOT NULL, \
  \"act\" text NOT NULL, \
  \"result\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL DEFAULT '', \
  \"report\" text NOT NULL, \
  \"at\" bigint NOT NULL)";

/// This build's Ordo release, as `$builds` and `$release_checks` record it.
pub fn release() -> &'static str {
    ordo_plan::artifact::ORDO_RELEASE
}

/// **A test-only release flag** (§2.8, Tests). In a debug build,
/// `ORDO_TEST_UNCOMPILABLE` names versions, comma-separated, that this
/// build refuses to compile, as a release that changed the language under
/// them would. A release build never reads it.
pub const TEST_UNCOMPILABLE: &str = "ORDO_TEST_UNCOMPILABLE";

/// Compile a stored version's source under this release. The one compile
/// of a stored version: the load at boot, a version loaded beside the
/// active one, and this check all come through here.
pub fn compile_stored(stored: &Artifact) -> Result<ordo_conformance::Fixture, String> {
    if cfg!(debug_assertions) {
        if let Ok(named) = std::env::var(TEST_UNCOMPILABLE) {
            if named.split(',').any(|v| v.trim() == stored.version) {
                return Err(format!(
                    "this release no longer accepts version `{}` ({TEST_UNCOMPILABLE}, a test \
                     build's flag)",
                    stored.version
                ));
            }
        }
    }
    ordo_conformance::Fixture::from_source(&stored.source, "")
}

/// One check that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// `identity`, `compile`, `auth`, `specs`, `shape`, `functions` or
    /// `plan`.
    pub check: &'static str,
    pub why: String,
}

/// One tenant in the report: its id and its name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tenant {
    pub space: String,
    pub name: String,
}

/// One checked version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub version: String,
    /// Why it is in the set: `active`, `pinned`, `new tenants`, `served`,
    /// `move target`, `on a stored path`.
    pub reasons: Vec<String>,
    /// The tenants it serves.
    pub tenants: Vec<Tenant>,
    pub failures: Vec<Failure>,
    /// What changed and is not a failure.
    pub notes: Vec<String>,
}

impl Checked {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// What one run found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub release: String,
    pub active: Option<String>,
    pub versions: Vec<Checked>,
    /// A name the set holds with no stored row: a setting or a target that
    /// names a version this deployment does not have.
    pub missing: Vec<(String, String)>,
    /// Each peer and reader, with the release it runs when that is known.
    pub neighbours: Vec<Neighbour>,
}

/// A peer this deployment reads, or a reader that reads it
/// (`plan-evolution-federation.md` §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Neighbour {
    /// `peer` or `reader`.
    pub kind: &'static str,
    /// The nickname its line gives it.
    pub name: String,
    /// A peer's schema, or a reader's database role.
    pub at: String,
    /// The Ordo release it last started under, when this deployment can
    /// read it.
    pub release: Option<String>,
    /// Why the release is not known, when it is not.
    pub unknown: String,
}

impl Neighbour {
    /// One line for a person.
    pub fn line(&self, this: &str) -> String {
        let place = if self.kind == "peer" {
            format!("schema `{}`", self.at)
        } else {
            format!("role `{}`", self.at)
        };
        match &self.release {
            Some(r) if r == this => {
                format!(
                    "{} {} ({place}): Ordo {r}, this release",
                    self.kind, self.name
                )
            }
            Some(r) => format!(
                "{} {} ({place}): Ordo {r}, ANOTHER RELEASE",
                self.kind, self.name
            ),
            None => format!("{} {} ({place}): {}", self.kind, self.name, self.unknown),
        }
    }
}

impl Report {
    pub fn passed(&self) -> bool {
        self.versions.iter().all(Checked::passed) && self.missing.is_empty()
    }

    /// The versions that failed.
    pub fn failing(&self) -> Vec<&Checked> {
        self.versions.iter().filter(|c| !c.passed()).collect()
    }

    /// Every tenant a failing version serves, by version: the tenants that
    /// close under this release, and so the ones to move first.
    pub fn closing(&self) -> Vec<(String, Vec<Tenant>)> {
        self.failing()
            .into_iter()
            .map(|c| (c.version.clone(), c.tenants.clone()))
            .collect()
    }

    pub fn result(&self) -> &'static str {
        if self.passed() {
            "pass"
        } else {
            "fail"
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "release": self.release,
            "active": self.active,
            "result": self.result(),
            "versions": self.versions.iter().map(|c| json!({
                "version": c.version,
                "reasons": c.reasons,
                "passed": c.passed(),
                "tenants": c.tenants.iter().map(|t| json!({"space": t.space, "name": t.name}))
                    .collect::<Vec<_>>(),
                "failures": c.failures.iter().map(|f| json!({"check": f.check, "why": f.why}))
                    .collect::<Vec<_>>(),
                "notes": c.notes,
            })).collect::<Vec<_>>(),
            "missing": self.missing.iter().map(|(v, why)| json!({"version": v, "named": why}))
                .collect::<Vec<_>>(),
            "neighbours": self.neighbours.iter().map(|n| json!({
                "kind": n.kind,
                "name": n.name,
                "at": n.at,
                "release": n.release,
                "otherRelease": n.release.as_ref().is_some_and(|r| r != &self.release),
                "unknown": n.unknown,
            })).collect::<Vec<_>>(),
        })
    }

    /// The report as lines for a person: each version, then what to do.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "release check: Ordo {} over {} stored version(s)",
            self.release,
            self.versions.len()
        )];
        for c in &self.versions {
            let mark = if c.passed() { "ok    " } else { "FAILS " };
            out.push(format!(
                "  {mark}{}  ({}; {} tenant(s))",
                c.version,
                c.reasons.join(", "),
                c.tenants.len()
            ));
            for f in &c.failures {
                out.push(format!("          {}: {}", f.check, f.why));
            }
            for n in &c.notes {
                out.push(format!("          note: {n}"));
            }
        }
        for (v, why) in &self.missing {
            out.push(format!(
                "  FAILS {v}  ({why}): no stored version has this name; set it to a stored \
                 version with `ordo versions`"
            ));
        }
        out.extend(neighbour_lines(&self.neighbours, &self.release));
        let failing = self.failing();
        if failing.is_empty() && self.missing.is_empty() {
            out.push(format!(
                "passed: every version compiles and loads under Ordo {}",
                self.release
            ));
            return out;
        }
        out.push(format!(
            "failed: under Ordo {} these tenants close, and every other tenant serves:",
            self.release
        ));
        for c in failing {
            let names: Vec<String> = c
                .tenants
                .iter()
                .map(|t| {
                    if t.name.is_empty() || t.name == t.space {
                        t.space.clone()
                    } else {
                        format!("{} ({})", t.space, t.name)
                    }
                })
                .collect();
            out.push(format!(
                "  {}: {}",
                c.version,
                if names.is_empty() {
                    "no tenant".to_string()
                } else {
                    names.join(", ")
                }
            ));
        }
        out.push(
            "move those tenants first, to a version that passes, with `ordo versions pin \
             <tenant> <version>` on the running deployment, then run the check again. To \
             upgrade anyway, record it: `ordo release-check --direct --override --reason \
             <why>`"
                .into(),
        );
        out
    }
}

// ── reading, tolerant of any earlier release's control database ─────────

// ── peers and readers ───────────────────────────────────────────────────

/// The peers and readers section: each one, then what the contract keeps.
/// Empty when there are none.
pub fn neighbour_lines(neighbours: &[Neighbour], this: &str) -> Vec<String> {
    use ordo_exec::sign_in_contract as c;
    let retired = c::retired_by(this);
    if neighbours.is_empty() && retired.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!(
        "peers and readers: the sign-in columns a reader reads stay readable for {} months \
         after a release changes one (`editions.load_window`)",
        ordo_plan::edition::LOAD_WINDOW_MONTHS
    )];
    for n in neighbours {
        out.push(format!("  {}", n.line(this)));
    }
    for col in retired {
        let r = col.retired.expect("retired");
        let now = r
            .replaced_by
            .map(|n| format!(" (now `{n}`)"))
            .unwrap_or_default();
        let until = c::kept_until(&r)
            .map(|d| d.to_string())
            .unwrap_or_else(|| "the window's end".into());
        out.push(format!(
            "  this release retires `{}.{}`{now}; every reader is still granted it until \
             Ordo releases dated {until}",
            col.table, col.column
        ));
    }
    out
}

/// **Each peer and each reader** of this deployment, and the release each
/// runs when it can be read (`plan-evolution-federation.md` §5, Q16).
///
/// Peers come from `schemas`, the checked versions' compiled programs: a
/// peer's schema is its `ORDO_PEER_<NAME>_SCHEMA`, read through `lookup`,
/// and its release from its `$ordorelease`, which it grants to its
/// readers. Readers come from `$readergrant`, the roles this deployment
/// grants to.
pub async fn neighbours(
    pool: &Pool,
    schemas: &[ordo_schema::Schema],
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Vec<Neighbour>, PoolError> {
    use ordo_exec::sign_in_contract::{release_of, PeerRelease};
    let mut out = Vec::new();
    let peers: BTreeSet<&String> = schemas.iter().flat_map(|s| s.peers.iter()).collect();
    for name in peers {
        let var = ordo_exec::peer::schema_env(name);
        let Some(pg) = lookup(&var).filter(|v| !v.trim().is_empty()) else {
            out.push(Neighbour {
                kind: "peer",
                name: name.clone(),
                at: "?".into(),
                release: None,
                unknown: format!(
                    "`{var}` is not set where the check runs, so its release is not read; run \
                     the check with the deployment's own settings"
                ),
            });
            continue;
        };
        let (release, unknown) = match release_of(pool, pg.trim()).await? {
            PeerRelease::Known(r) => (Some(r), String::new()),
            PeerRelease::Unrecorded => (
                None,
                format!("an Ordo release before {}, which records none", release()),
            ),
            PeerRelease::NotGranted => (
                None,
                "its release is not granted to this deployment yet; the peer grants it at \
                 its next start"
                    .to_string(),
            ),
        };
        out.push(Neighbour {
            kind: "peer",
            name: name.clone(),
            at: pg.trim().to_string(),
            release,
            unknown,
        });
    }
    for r in rows_or_none(
        pool,
        r#"SELECT "reader", "role" FROM "$readergrant" ORDER BY 1, 2"#,
    )
    .await?
    {
        let (Some(name), Some(role)) = (col(&r, 0), col(&r, 1)) else {
            continue;
        };
        // The reader grants its release column to the role that owns this
        // deployment's schema (owner ruling 2026-11-25), so its schema is
        // the one its role owns a `$ordorelease` in.
        let own = pool.schema().to_string();
        let schemas: Vec<String> = ordo_exec::sign_in_contract::schemas_owned_by(pool, &role)
            .await?
            .into_iter()
            .filter(|s| *s != own)
            .collect();
        let (found, unknown) = match schemas.first() {
            None => (
                None,
                "it records no release this deployment can find; it records one at its \
                 next start"
                    .to_string(),
            ),
            Some(pg) => match release_of(pool, pg).await? {
                PeerRelease::Known(r) => (Some(r), String::new()),
                PeerRelease::Unrecorded => (
                    None,
                    format!("an Ordo release before {}, which records none", release()),
                ),
                PeerRelease::NotGranted => (
                    None,
                    "it has not granted its release to this deployment yet; it grants it \
                     at its next start"
                        .to_string(),
                ),
            },
        };
        out.push(Neighbour {
            kind: "reader",
            name,
            at: role,
            release: found,
            unknown,
        });
    }
    Ok(out)
}

/// Rows of `sql`, or none when a table or column it reads is not there.
async fn rows_or_none(pool: &Pool, sql: &str) -> Result<Vec<Vec<Option<String>>>, PoolError> {
    match pool.rows(sql).await {
        Ok(r) => Ok(r),
        Err(e) if e.is_missing_table() || e.is_missing_column() => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn col(r: &[Option<String>], i: usize) -> Option<String> {
    r.get(i).cloned().flatten()
}

/// The active version, from `$activations`.
async fn active_version(pool: &Pool) -> Result<Option<String>, PoolError> {
    Ok(rows_or_none(
        pool,
        "SELECT \"version\" FROM \"$activations\" ORDER BY \"seq\" DESC LIMIT 1",
    )
    .await?
    .first()
    .and_then(|r| col(r, 0)))
}

/// A stored version, read with the columns every release has had.
async fn stored(pool: &Pool, version: &str) -> Result<Option<Artifact>, PoolError> {
    let rows = rows_or_none(
        pool,
        &format!(
            "SELECT \"version\", \"artifact\", \"source\", \"ddl\", \"applied_at\", \"actor\" \
             FROM \"$artifacts\" WHERE \"version\" = {}",
            sql_lit(version)
        ),
    )
    .await?;
    Ok(rows.first().and_then(|r| {
        Some(Artifact {
            version: col(r, 0)?,
            artifact: col(r, 1)?,
            source: col(r, 2)?,
            ddl: col(r, 3)?,
            applied_at: ordo_commandlog::Timestamp(col(r, 4)?.parse().ok()?),
            actor: col(r, 5).unwrap_or_default(),
        })
    }))
}

/// The source fingerprint recorded for a version. `None`: not recorded
/// yet, which the first boot of a release that records it fills in.
async fn recorded_source_id(pool: &Pool, version: &str) -> Result<Option<String>, PoolError> {
    Ok(rows_or_none(
        pool,
        &format!(
            "SELECT \"source_id\" FROM \"$artifacts\" WHERE \"version\" = {}",
            sql_lit(version)
        ),
    )
    .await?
    .first()
    .and_then(|r| col(r, 0))
    .filter(|s| !s.is_empty()))
}

/// A setting's version name: `#n` read as the version with that order
/// number, any other name as itself.
async fn name_for_ordinal(pool: &Pool, name: &str) -> Result<String, PoolError> {
    let Some(n) = name
        .strip_prefix('#')
        .and_then(|n| n.trim().parse::<u64>().ok())
    else {
        return Ok(name.to_string());
    };
    Ok(rows_or_none(
        pool,
        &format!("SELECT \"version\" FROM \"$artifacts\" WHERE \"ordinal\" = {n}"),
    )
    .await?
    .first()
    .and_then(|r| col(r, 0))
    .unwrap_or_else(|| name.to_string()))
}

/// Every tenant, with the version it is served: its pin, else the active
/// version. Only tenants that are being built, serving or held.
async fn tenants(
    pool: &Pool,
    active: Option<&str>,
) -> Result<BTreeMap<String, Vec<Tenant>>, PoolError> {
    // With the pin column, else without it (a database from before pins).
    let mut rows = rows_or_none(
        pool,
        "SELECT \"$space\", \"name\", \"version\" FROM \"$spaces\" \
         WHERE \"state\" IN ('provisioning', 'active', 'paused') ORDER BY \"$space\"",
    )
    .await?;
    if rows.is_empty() {
        rows = rows_or_none(
            pool,
            "SELECT \"$space\", \"name\", NULL FROM \"$spaces\" ORDER BY \"$space\"",
        )
        .await?;
    }
    let mut out: BTreeMap<String, Vec<Tenant>> = BTreeMap::new();
    for r in rows {
        let Some(space) = col(&r, 0) else { continue };
        let Some(version) = col(&r, 2).or_else(|| active.map(str::to_string)) else {
            continue;
        };
        out.entry(version).or_default().push(Tenant {
            name: col(&r, 1).unwrap_or_default(),
            space,
        });
    }
    Ok(out)
}

/// The version settings' current values, `(setting, value)`.
async fn settings(pool: &Pool) -> Result<BTreeMap<String, String>, PoolError> {
    let rows = rows_or_none(
        pool,
        "SELECT DISTINCT ON (\"setting\") \"setting\", \"value\" FROM \"$version_settings\" \
         ORDER BY \"setting\", \"seq\" DESC",
    )
    .await?;
    Ok(rows
        .iter()
        .filter_map(|r| Some((col(r, 0)?, col(r, 1).unwrap_or_default())))
        .filter(|(_, v)| !v.trim().is_empty())
        .collect())
}

/// Every version a move points at: open moves, tenants' own targets, and
/// revokes' `--move-to`.
async fn move_targets(pool: &Pool) -> Result<BTreeSet<String>, PoolError> {
    let mut out = BTreeSet::new();
    for sql in [
        "SELECT \"to\" FROM \"$moves\"",
        "SELECT \"version\" FROM \"$move_targets\"",
        "SELECT \"move_to\" FROM \"$revocations\" WHERE \"move_to\" IS NOT NULL",
    ] {
        for r in rows_or_none(pool, sql).await? {
            if let Some(v) = col(&r, 0).filter(|v| !v.is_empty()) {
                out.insert(v);
            }
        }
    }
    Ok(out)
}

/// Every stored plan's edge and digest: `(from, to, digest, options)`.
async fn plan_edges(pool: &Pool) -> Result<Vec<(String, String, String, String)>, PoolError> {
    let rows = rows_or_none(
        pool,
        "SELECT \"from_version\", \"to_version\", \"digest\", \"options\" FROM \"$plans\" \
         ORDER BY \"seq\"",
    )
    .await?;
    // The newest plan per edge is the one a path runs.
    let mut newest: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
    for r in rows {
        if let (Some(f), Some(t), Some(d)) = (col(&r, 0), col(&r, 1), col(&r, 2)) {
            newest.insert((f, t), (d, col(&r, 3).unwrap_or_default()));
        }
    }
    Ok(newest
        .into_iter()
        .map(|((f, t), (d, o))| (f, t, d, o))
        .collect())
}

/// **The checked set**: version to why it is in it, and the tenants by
/// version. Read only.
pub async fn checked_set(
    pool: &Pool,
) -> Result<
    (
        Option<String>,
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, Vec<Tenant>>,
        Vec<(String, String, String, String)>,
    ),
    PoolError,
> {
    let active = active_version(pool).await?;
    let by_version = tenants(pool, active.as_deref()).await?;
    let mut set: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    fn add(set: &mut BTreeMap<String, BTreeSet<String>>, v: String, why: &str) {
        set.entry(v).or_default().insert(why.to_string());
    }
    if let Some(a) = &active {
        add(&mut set, a.clone(), "active");
    }
    for v in by_version.keys() {
        add(&mut set, v.clone(), "pinned");
    }
    let s = settings(pool).await?;
    if let Some(v) = s.get("versions.new_tenant") {
        add(
            &mut set,
            name_for_ordinal(pool, v.trim()).await?,
            "new tenants",
        );
    }
    if let Some(list) = s.get("versions.served") {
        if list.trim() != "in-use" {
            for v in list.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                add(&mut set, name_for_ordinal(pool, v).await?, "served");
            }
        }
    }
    for v in move_targets(pool).await? {
        add(&mut set, v, "move target");
    }
    // Every version on a stored path between any 2 in the set: the
    // versions reachable from one of them that reach another.
    let edges = plan_edges(pool).await?;
    let ends: BTreeSet<String> = set.keys().cloned().collect();
    let reach = |from: &str, forward: bool| -> BTreeSet<String> {
        let mut seen = BTreeSet::from([from.to_string()]);
        let mut todo = vec![from.to_string()];
        while let Some(at) = todo.pop() {
            for (f, t, _, _) in &edges {
                let (a, b) = if forward { (f, t) } else { (t, f) };
                if *a == at && seen.insert(b.clone()) {
                    todo.push(b.clone());
                }
            }
        }
        seen
    };
    let mut on_path = BTreeSet::new();
    for x in &ends {
        let after = reach(x, true);
        for y in ends.iter().filter(|y| *y != x && after.contains(*y)) {
            let before = reach(y, false);
            on_path.extend(after.intersection(&before).cloned());
        }
    }
    let on_path: Vec<String> = on_path.difference(&ends).cloned().collect();
    for v in on_path {
        add(&mut set, v, "on a stored path");
    }
    // The plans the path walks: each edge whose ends are both in the set.
    let edges = edges
        .into_iter()
        .filter(|(f, t, _, _)| set.contains_key(f) && set.contains_key(t))
        .collect();
    Ok((active, set, by_version, edges))
}

/// Classify a compile refusal: an `auth` block outside this release's
/// ranges is its own check.
fn compile_check(why: &str) -> &'static str {
    use ordo_plan::edition as e;
    if why.contains(e::EDITION_EXPIRED) || why.contains(e::EDITION_UNKNOWN) {
        "edition"
    } else if why.contains("auth_duration_range") || why.contains("invalid_auth") {
        "auth"
    } else {
        "compile"
    }
}

/// **Check one stored version** under this release.
pub async fn check_version(
    pool: &Pool,
    cfg: &PoolConfig,
    stored: &Artifact,
) -> (Vec<Failure>, Vec<String>) {
    let mut failures = Vec::new();
    let mut notes = Vec::new();
    let fail = |check, why: String| Failure { check, why };

    // 1. Identity: the fingerprint the row records, recomputed.
    {
        use ordo_plan::artifact as a;
        let doc: Value = serde_json::from_str(&stored.artifact).unwrap_or_default();
        let computed = a::source_identity(
            &stored.source,
            ordo_plan::edition::of_source(&stored.source),
            &a::code_hashes(&doc),
        );
        match recorded_source_id(pool, &stored.version).await {
            Ok(Some(recorded)) if recorded != computed => failures.push(fail(
                "identity",
                crate::deploy::LoadRefusal::Digest {
                    version: stored.version.clone(),
                    stored: recorded,
                    computed,
                }
                .to_string(),
            )),
            Ok(Some(_)) => {}
            Ok(None) => notes.push(
                "no source fingerprint is recorded yet; the first boot of this release \
                 records it from the stored source"
                    .into(),
            ),
            Err(e) => failures.push(fail("identity", format!("not read: {e}"))),
        }
    }

    // The edition: an old one near its window's end is called out, so the
    // developer runs `ordo fix` before a release drops it. One outside the
    // window fails the compile below, under the `edition` check.
    {
        use ordo_plan::edition as e;
        let editions = e::Editions::this_release();
        let n = e::of_source(&stored.source);
        if let e::Standing::Loadable { ends, near } = editions.standing(n) {
            let current = editions.current();
            let when = if near { "soon: " } else { "" };
            notes.push(format!(
                "{when}it is written in edition {n}, which Ordo releases dated up to {ends} \
                 load (`editions.load_window`). Run `ordo fix <dir> --write` on its source \
                 to move it to edition {current}, then apply it"
            ));
        }
    }

    // 2 and 3. Compile, with the `auth` ranges inside it.
    let compiled = match compile_stored(stored) {
        Ok(f) => f,
        Err(why) => {
            failures.push(fail(compile_check(&why), why));
            return (failures, notes);
        }
    };

    // The release moving the build: a note, never a failure (§1.1).
    match crate::schema_apply::Program::from_artifact(stored) {
        Ok(p) if !p.builds_as(stored) => notes.push(format!(
            "Ordo {} builds it to other bytes than were stored; it stays version `{}` with \
             a new build",
            release(),
            stored.version
        )),
        Ok(_) | Err(_) => {}
    }

    // 4. Specs: the door's registration, and the platform's reserved names.
    let specs = crate::build_specs(&compiled);
    if let Err(e) = ordo_api::MapRegistry::new(specs) {
        failures.push(fail(
            "specs",
            format!("the door refuses to register an op: {e}"),
        ));
    }
    for s in crate::platform_ops::specs() {
        if compiled.registry.get(&s.name).is_some() {
            failures.push(fail(
                "specs",
                format!(
                    "it compiles an op named `{}`, which Ordo {} serves as a platform op",
                    s.name,
                    release()
                ),
            ));
        }
    }

    // 5. The shape gate over an empty shadow of the stored DDL.
    let mut gated = compiled;
    match ordo_exec::peer::resolve(&mut gated.schema, pool.schema()) {
        Ok(()) => {
            if let Err(e) = crate::shape::verify_registry(&gated, cfg, &stored.ddl).await {
                failures.push(fail("shape", e.to_string()));
            }
        }
        Err(e) => notes.push(format!(
            "the shape gate was not run: an external's schema is this environment's \
             setting, and the boot checks it ({e})"
        )),
    }

    // 6. Functions: code by hash, and the table.
    if !gated.schema.functions.is_empty() {
        let doc: Value = serde_json::from_str(&stored.artifact).unwrap_or_default();
        let mut code = BTreeMap::new();
        for (_, hash) in ordo_plan::artifact::code_hashes(&doc) {
            match function_code(pool, &hash).await {
                Ok(Some(bytes)) => {
                    let got = ordo_plan::artifact::code_hash(&bytes);
                    if got != hash {
                        failures.push(fail(
                            "functions",
                            format!("code {hash} reads back as {got}: the row was changed"),
                        ));
                    }
                    code.insert(hash, bytes);
                }
                Ok(None) => {}
                Err(e) => failures.push(fail("functions", format!("not read: {e}"))),
            }
        }
        if let Err(why) =
            crate::functions::FnTable::build(&gated.schema, &doc, &stored.version, &code)
        {
            failures.push(fail("functions", why));
        }
    }
    (failures, notes)
}

/// A function's code by hash, without the table's setup.
async fn function_code(pool: &Pool, hash: &str) -> Result<Option<Vec<u8>>, PoolError> {
    Ok(rows_or_none(
        pool,
        &format!(
            "SELECT \"code\" FROM \"$fncode\" WHERE \"hash\" = {}",
            sql_lit(hash)
        ),
    )
    .await?
    .first()
    .and_then(|r| col(r, 0))
    .map(String::into_bytes))
}

/// **Run the check.** Reads the control database; writes nothing.
pub async fn run(pool: &Pool, cfg: &PoolConfig) -> Result<Report, PoolError> {
    let (active, set, by_version, edges) = checked_set(pool).await?;
    let mut report = Report {
        release: release().to_string(),
        active,
        versions: Vec::new(),
        missing: Vec::new(),
        neighbours: Vec::new(),
    };
    let mut artifacts: BTreeMap<String, Artifact> = BTreeMap::new();
    for (version, reasons) in &set {
        let Some(a) = stored(pool, version).await? else {
            report.missing.push((
                version.clone(),
                reasons.iter().cloned().collect::<Vec<_>>().join(", "),
            ));
            continue;
        };
        let (failures, notes) = check_version(pool, cfg, &a).await;
        report.versions.push(Checked {
            version: version.clone(),
            reasons: reasons.iter().cloned().collect(),
            tenants: by_version.get(version).cloned().unwrap_or_default(),
            failures,
            notes,
        });
        artifacts.insert(version.clone(), a);
    }
    // 7. Each stored plan between 2 checked versions re-renders to its
    // digest. Failed on its `to` version, whose tenants it would move.
    for (from, to, digest, options) in edges {
        let (Some(a), Some(b)) = (artifacts.get(&from), artifacts.get(&to)) else {
            continue;
        };
        let compiles = |v: &str| {
            report
                .versions
                .iter()
                .any(|c| c.version == v && !c.failures.iter().any(|f| f.check == "compile"))
        };
        if !compiles(&from) || !compiles(&to) {
            continue;
        }
        let why = match crate::evolve::parse_options(&options) {
            Err(e) => Some(format!("its options do not read: {e}")),
            Ok(choices) => match crate::evolve::render_stored(pool, a, b, &choices, 0).await {
                Ok(r) if r.digest == digest => None,
                Ok(r) => Some(format!(
                    "the stored plan from `{from}` to `{to}` renders to {} under Ordo {}, not \
                     its digest {digest}. Render it again with `ordo evolve --from {from} --to \
                     {to}`",
                    r.digest,
                    release()
                )),
                Err(e) => Some(format!(
                    "the stored plan from `{from}` to `{to}` does not render: {e}"
                )),
            },
        };
        if let Some(why) = why {
            if let Some(c) = report.versions.iter_mut().find(|c| c.version == to) {
                c.failures.push(Failure { check: "plan", why });
            }
        }
    }
    // The peers every checked version that compiles names, and the
    // readers this deployment grants to.
    let schemas: Vec<ordo_schema::Schema> = artifacts
        .values()
        .filter_map(|a| compile_stored(a).ok())
        .map(|f| f.schema)
        .collect();
    report.neighbours = neighbours(pool, &schemas, ordo_exec::schema_setting).await?;
    Ok(report)
}

// ── the record ──────────────────────────────────────────────────────────

/// How a check was run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// `ordo release-check`, before the image changes.
    Command,
    /// The first boot of a release.
    Boot,
    /// The deployment owner's override.
    Override,
}

impl Act {
    pub fn as_str(self) -> &'static str {
        match self {
            Act::Command => "check",
            Act::Boot => "boot",
            Act::Override => "override",
        }
    }
}

/// One `$release_checks` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub seq: i64,
    pub release: String,
    pub act: String,
    pub result: String,
    pub actor: String,
    pub reason: String,
    pub report: Value,
    pub at: u64,
}

async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    // Two writers creating it at once can collide inside Postgres even
    // with `IF NOT EXISTS`; the second try finds it.
    match pool.rows(DDL).await {
        Ok(_) => Ok(()),
        Err(_) => pool.rows(DDL).await.map(|_| ()),
    }
}

/// Write one row: the check, the boot's run, or an override.
pub async fn record(
    pool: &Pool,
    report: &Report,
    act: Act,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<i64, PoolError> {
    ensure_table(pool).await?;
    let rows = pool
        .rows(&format!(
            "INSERT INTO \"$release_checks\" (\"release\", \"act\", \"result\", \"actor\", \
             \"reason\", \"report\", \"at\") VALUES ({}, {}, {}, {}, {}, {}, {at}) \
             RETURNING \"seq\"",
            sql_lit(&report.release),
            sql_lit(act.as_str()),
            sql_lit(report.result()),
            sql_lit(actor),
            sql_lit(reason),
            sql_lit(&report.to_json().to_string()),
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| col(r, 0))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// Every row for one release, oldest first. None when the table is not
/// there.
pub async fn history(pool: &Pool, release: &str) -> Result<Vec<Recorded>, PoolError> {
    let rows = rows_or_none(
        pool,
        &format!(
            "SELECT \"seq\", \"release\", \"act\", \"result\", \"actor\", \"reason\", \
             \"report\", \"at\" FROM \"$release_checks\" WHERE \"release\" = {} ORDER BY \"seq\"",
            sql_lit(release)
        ),
    )
    .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(Recorded {
                seq: col(r, 0)?.parse().ok()?,
                release: col(r, 1)?,
                act: col(r, 2)?,
                result: col(r, 3)?,
                actor: col(r, 4).unwrap_or_default(),
                reason: col(r, 5).unwrap_or_default(),
                report: serde_json::from_str(&col(r, 6).unwrap_or_default()).unwrap_or(Value::Null),
                at: col(r, 7)?.parse().ok()?,
            })
        })
        .collect())
}

/// **The override**: the deployment owner upgrades to this release with
/// the versions `report` names failing. A reason is required. Answers the
/// row's sequence number.
pub async fn record_override(
    pool: &Pool,
    report: &Report,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<i64, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(
            "an override records why: add `--reason <why>`, for example `--reason \"security \
             fix\"`"
                .into(),
        );
    }
    record(pool, report, Act::Override, actor, reason, at)
        .await
        .map_err(|e| e.to_string())
}

/// **The boot's run** (R3). At the first boot of a release, run the check,
/// log each line, and record it. Never refuses: a failing version's
/// tenants are closed by its load refusal, and every other tenant serves.
/// A release already checked at a boot is not checked again.
pub async fn at_boot(pool: &Pool, cfg: &PoolConfig, at: u64) -> Vec<String> {
    let earlier = match history(pool, release()).await {
        Ok(h) => h,
        Err(e) => return vec![format!("release check: not run at boot ({e})")],
    };
    if earlier.iter().any(|r| r.act == Act::Boot.as_str()) {
        return Vec::new();
    }
    let report = match run(pool, cfg).await {
        Ok(r) => r,
        Err(e) => return vec![format!("release check: not run at boot ({e})")],
    };
    let mut out: Vec<String> = report.lines();
    if !report.passed() {
        match earlier
            .iter()
            .rev()
            .find(|r| r.act == Act::Override.as_str())
        {
            Some(o) => out.push(format!(
                "release check: the override {} recorded is in force: {}",
                o.actor, o.reason
            )),
            None => out.push(
                "release check: no override was recorded for this release; the failing \
                 versions' tenants are closed until they move"
                    .into(),
            ),
        }
    }
    if let Err(e) = record(pool, &report, Act::Boot, "boot", "", at).await {
        out.push(format!("release check: not recorded ({e})"));
    }
    out
}

/// Whether a refusal of the active version at boot is one a release can
/// cause, and so closes that version's tenants rather than stopping the
/// boot (owner, 2026-11-06). A setting, a tampered row or an unreachable
/// database still refuses, naming its fix.
pub fn closes_only_its_tenants(e: &crate::deploy::LoadRefusal) -> bool {
    use crate::deploy::LoadRefusal as L;
    matches!(
        e,
        L::Compile { .. } | L::Specs { .. } | L::ReservedOpName { .. } | L::Shape { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_auth_range_refusal_is_reported_as_the_auth_check() {
        assert_eq!(
            compile_check("auth_duration_range: `token 9d` is outside"),
            "auth"
        );
        assert_eq!(compile_check("invalid_auth: a method listed twice"), "auth");
        assert_eq!(compile_check("parse error at line 3"), "compile");
    }

    #[test]
    fn a_failing_report_names_its_version_its_tenants_and_the_override() {
        let r = Report {
            release: "9.9.9".into(),
            active: Some("0.2.0".into()),
            versions: vec![
                Checked {
                    version: "0.1.0".into(),
                    reasons: vec!["pinned".into()],
                    tenants: vec![Tenant {
                        space: "spc_a".into(),
                        name: "Acme".into(),
                    }],
                    failures: vec![Failure {
                        check: "compile",
                        why: "no".into(),
                    }],
                    notes: vec![],
                },
                Checked {
                    version: "0.2.0".into(),
                    reasons: vec!["active".into(), "pinned".into()],
                    tenants: vec![],
                    failures: vec![],
                    notes: vec![],
                },
            ],
            missing: vec![],
            neighbours: vec![
                Neighbour {
                    kind: "peer",
                    name: "journal".into(),
                    at: "journal_db".into(),
                    release: Some("9.8.0".into()),
                    unknown: String::new(),
                },
                Neighbour {
                    kind: "reader",
                    name: "viewer".into(),
                    at: "viewer_app".into(),
                    release: None,
                    unknown: "not visible".into(),
                },
            ],
        };
        assert!(!r.passed());
        let text = r.lines().join("\n");
        assert!(text.contains("FAILS 0.1.0"), "{text}");
        assert!(text.contains("spc_a (Acme)"), "{text}");
        assert!(text.contains("--override --reason"), "{text}");
        assert!(text.contains("ok    0.2.0"), "{text}");
        // Peers and readers are named, and never fail the check.
        assert!(
            text.contains("peer journal (schema `journal_db`): Ordo 9.8.0, ANOTHER RELEASE"),
            "{text}"
        );
        assert!(
            text.contains("reader viewer (role `viewer_app`): not visible"),
            "{text}"
        );
        assert!(text.contains("`editions.load_window`"), "{text}");
        assert_eq!(r.to_json()["result"], "fail");
        assert_eq!(r.to_json()["neighbours"][0]["otherRelease"], true);
    }
}
