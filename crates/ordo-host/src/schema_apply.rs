//! Applying a program's schema to a deployment's database: the plan, the
//! probes, the class-A apply, and the first create.
//! `plan-schema-evolution.md` increments 1 and 2.
//!
//! # This belongs to the service
//!
//! Owner-ruled 2026-09-20: *"the apply belongs to the service as opposed to
//! a run from the client to the database."* So this module takes a
//! [`Pool`] and a compiled [`Program`] and returns structured outcomes; it
//! reads no environment, prints nothing, prompts for nothing, and shells
//! out to nothing. `ordo migrate --apply` and `ordo apply` are thin callers
//! that parse arguments, render what comes back and set an exit code, and
//! they exist to confirm the logic and to test it. A hosted control plane
//! (checklist 18l) calls the same functions from behind an owner-gated
//! platform op, and adds the door: who may apply, from where, with which
//! actor attested rather than asserted.
//!
//! # What it does, in order
//!
//! 1. [`plan`]: compile nothing (the caller did), read the serving
//!    artifact and the live shape, classify every difference
//!    (`ordo_schema::diff`) and probe every class-C and class-B change
//!    (`ordo_exec::migrate`). Read-only.
//! 2. [`schema_state`]: is the target empty, a match, behind, or a
//!    registry over no tables?
//! 3. [`create_schema`]: a first apply against an empty target runs the
//!    whole DDL through the one path the platform already had for a fresh
//!    database, `ordo_exec::spacedb::apply_schema`, which
//!    `provision::prepare_target` runs for a space's own database.
//! 4. [`triage`]: for a deployed schema, decide per change the statements
//!    to send or the reason not to. Class A applies; a foreign key, an
//!    enum pin or a check onto a table that exists is added `NOT VALID`
//!    and validated as its own step; a unique applies when it constrains
//!    no existing data; everything else is refused by name with the
//!    increment that owns it.
//! 5. [`execute`]: tables and alters in one transaction, then each
//!    `VALIDATE CONSTRAINT` on its own, then each
//!    `CREATE INDEX CONCURRENTLY` on its own, read back for validity.
//! 6. [`record`]: a `$migrations` row and a deployment-scoped
//!    `SchemaMigration` security event, whether the apply landed or not.
//!
//! [`apply`] composes 1 through 6 into one call and one structured answer.
//!
//! # Constraints onto a table that exists (§3.7)
//!
//! `ALTER TABLE … ADD CONSTRAINT` takes an `ACCESS EXCLUSIVE` lock and, in
//! its plain form, scans every row under it. Added `NOT VALID` it takes the
//! lock for a moment and scans nothing; new writes are checked from then
//! on. `VALIDATE CONSTRAINT` then scans the existing rows under a `SHARE
//! UPDATE EXCLUSIVE` lock, which lets reads and writes continue. So a
//! foreign key, an enum pin or a check onto a table that exists is two
//! steps: the `NOT VALID` add inside the transaction with the other
//! alters, and the validate after it commits, on its own. A brand-new
//! table keeps its constraints inline: there is nothing to scan and nobody
//! to lock out.
//!
//! The plan's probe has already checked the rows, so validation fails only
//! when a row that breaks the constraint was written between the probe and
//! the add. Then the apply fails naming the constraint and the table, and
//! **drops the constraint again** ([`ExecFailure::Validate`]). The table
//! holds what the transaction committed and nothing of the constraint,
//! which is what the plan's reverse (drop what was added) leaves too, and
//! the next apply plans the constraint afresh: its probe names the rows.
//! Leaving a `NOT VALID` constraint in place was the alternative, and it
//! was refused because the live shape would read it as present and no
//! later apply would validate it.
//!
//! # The one rule beyond class A, recorded so it is a decision
//!
//! Increment 2 is *"class A only, refusing everything else by name"*. One
//! extension was taken and recorded in `corpus-rulings.md`: a class-C
//! constraint is applied when the table is empty, or when every column it
//! names arrives in this same apply and its probe is clean by
//! construction. Those constraints are the column's own declaration; a
//! fresh `CREATE TABLE` would have carried them, and a column that arrived
//! without its foreign key would be a column the artifact asserts and the
//! gate refuses. With the `NOT VALID` path built, that rule now decides
//! only uniques, which have no `NOT VALID` form: a unique over existing
//! rows is refused with its statement carried, until §3.7's index built
//! `CONCURRENTLY` and attached with `USING INDEX` is built.

use std::collections::{BTreeMap, BTreeSet};

use std::time::Duration;

use ordo_exec::migrate;
use ordo_exec::{Pool, PoolError};
use ordo_schema::diff::{Change, Class, PlacesChoice};
use ordo_schema::{LiveShape, Schema};

/// The schema of a live deployment. Not configurable: `live_drift` compares
/// the shadow against the literal `"public"`, and a plan that looked
/// somewhere else would classify against a schema the gate never checks.
pub const LIVE: &str = "public";

/// How many offending values a probe carries. Enough to recognise the shape
/// of the problem; not so many that the refusal scrolls past the verdict.
const SAMPLE: usize = 8;

/// The chunk the throughput measurement scans. The backfill's own unit of
/// work in increment 3, so the number means something there too.
const CHUNK: usize = 5_000;

// ── the program ─────────────────────────────────────────────────────────

/// A compiled program, in the four forms an apply needs.
#[derive(Debug, Clone)]
pub struct Program {
    pub schema: Schema,
    /// The byte-stable artifact document.
    pub doc: serde_json::Value,
    pub ddl: String,
    /// `ordo_plan::artifact::program_version` of the document and the DDL.
    pub version: String,
    pub source: String,
}

impl Program {
    /// Compile one program text through the same pipeline the load gate
    /// uses (`ordo_conformance::Fixture::from_source`), so what is planned
    /// against a database is what a node would load.
    pub fn compile(source: &str) -> Result<Program, String> {
        let fixture = crate::Fixture::from_source(source, "")?;
        let doc = ordo_plan::artifact::program_artifact(&fixture.schema, &fixture.registry)
            .map_err(|e| format!("artifact: {e}"))?;
        let ddl = ordo_schema::ddl_for(&fixture.schema).map_err(|e| format!("ddl: {e}"))?;
        let version = ordo_plan::artifact::program_version(&doc, &ddl);
        Ok(Program {
            schema: fixture.schema,
            doc,
            ddl,
            version,
            source: source.to_string(),
        })
    }

    /// Fills in each function's `code_hash` from `hashes` (code path to
    /// hash) and takes the version again, so a change to a function's code
    /// alone is a new version (`plan-backend-functions.md` §4.3). A program
    /// with no functions is unchanged.
    pub fn bind_code(&mut self, hashes: &BTreeMap<String, String>) -> Result<(), String> {
        if self.schema.functions.is_empty() {
            return Ok(());
        }
        ordo_plan::artifact::bind_function_code(&mut self.doc, hashes)?;
        self.version = ordo_plan::artifact::program_version(&self.doc, &self.ddl);
        Ok(())
    }

    /// The program a stored artifact holds: its source compiled, with each
    /// function's code hash read back from the stored document. Compiling
    /// the source alone loses the hashes, and with them the version.
    pub fn from_artifact(a: &ordo_exec::registry::Artifact) -> Result<Program, String> {
        let mut program = Program::compile(&a.source)?;
        if program.schema.functions.is_empty() {
            return Ok(program);
        }
        let stored: serde_json::Value = serde_json::from_str(&a.artifact)
            .map_err(|e| format!("the stored artifact {} is not JSON: {e}", a.version))?;
        let hashes: BTreeMap<String, String> = stored["functions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| {
                Some((
                    f["code"].as_str()?.to_string(),
                    f["code_hash"].as_str()?.to_string(),
                ))
            })
            .collect();
        program.bind_code(&hashes)?;
        Ok(program)
    }
}

/// The artifact the deployment is serving, or `None` when there is no
/// registry or nothing is activated.
///
/// Errors are folded into `None` on purpose: a database with no `$artifacts`
/// table is a database that has never been applied to, which is a state a
/// plan must report on rather than fail at.
pub async fn deployed_artifact(pool: &Pool) -> Option<ordo_exec::registry::Artifact> {
    let v = ordo_exec::registry::active_version(pool).await.ok()??;
    ordo_exec::registry::get(pool, &v).await.ok()?
}

fn recompile(a: &ordo_exec::registry::Artifact) -> Option<Schema> {
    crate::Fixture::from_source(&a.source, "")
        .ok()
        .map(|f| f.schema)
}

// ── what the developer stated ───────────────────────────────────────────

/// What a developer stated for a change the plan cannot decide alone.
/// Today one kind: what happens to a stored money or decimal field's
/// amounts when its places change (25aa, ruled 2026-10-05).
///
/// Carried by the call (`ordo migrate --apply --places`), because the
/// stored plan that should hold every choice (`plan-evolution-plans.md`
/// §3.1 and §3.2) is not built. Once applied, each choice is written to
/// `$places` with the change, and a tenant's database brought forward later
/// reads it from the deployment's own database ([`bring_forward_from`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choices {
    /// `(entity, field)` to the stated choice.
    pub places: BTreeMap<(String, String), PlacesChoice>,
}

impl Choices {
    /// Parse `invoices.total=rescale,fees.rate=keep`.
    pub fn parse_places(spec: &str) -> Result<Choices, String> {
        let mut out = Choices::default();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (target, choice) = part.split_once('=').ok_or_else(|| {
                format!("`{part}`: write `<entity>.<field>=rescale` or `<entity>.<field>=keep`")
            })?;
            let (entity, field) = target
                .trim()
                .split_once('.')
                .ok_or_else(|| format!("`{target}`: name the field as `<entity>.<field>`"))?;
            let choice = PlacesChoice::parse(choice)
                .ok_or_else(|| format!("`{choice}`: the choice is `rescale` or `keep`"))?;
            out.places.insert(
                (entity.trim().to_string(), field.trim().to_string()),
                choice,
            );
        }
        Ok(out)
    }

    pub fn places_for(&self, entity: &str, field: &str) -> Option<PlacesChoice> {
        self.places
            .get(&(entity.to_string(), field.to_string()))
            .copied()
    }
}

// ── the plan, as data ───────────────────────────────────────────────────

/// A probe's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The change has no probe.
    None,
    /// Clean, and what was asked.
    Clean(String),
    /// Non-empty: what was asked, and the offending values.
    Dirty { what: String, lines: Vec<String> },
}

/// One change with its probe and its rulings.
#[derive(Debug, Clone)]
pub struct Assessed {
    pub change: Change,
    /// Refusal classes raised against this change: a ruling
    /// (`unseal_refused`), a deploy-order violation
    /// (`drop_read_by_active_op`), or the probe's own (`probe_not_empty`,
    /// `probe_unavailable — <why>`).
    pub refused: Vec<String>,
    /// §5 D3: the serving artifact's ops that still name what this drops.
    pub readers: Vec<String>,
    pub probe: Option<Result<Probe, String>>,
    /// Class B's freeze-estimate inputs: the entity's extent and the
    /// measured scan rate.
    pub extent: Option<(migrate::Extent, Option<f64>)>,
}

/// Everything the plan learned, before any of it is rendered. A report
/// prints it; the apply consumes it. One walk over the database, never
/// two, so what is applied is exactly what was reported.
#[derive(Debug, Clone)]
pub struct Plan {
    pub program: Program,
    pub deployed: Option<ordo_exec::registry::Artifact>,
    /// The serving artifact's source, recompiled. `None` when nothing is
    /// deployed or it no longer compiles.
    pub prior: Option<Schema>,
    pub prior_doc: Option<serde_json::Value>,
    pub live: LiveShape,
    pub assessed: Vec<Assessed>,
    /// Every refusal, `class on entity`, in report order.
    pub refusals: Vec<String>,
    pub destructive: usize,
    /// What the developer stated, which the triage applies.
    pub choices: Choices,
}

impl Plan {
    pub fn changes(&self) -> impl Iterator<Item = &Change> {
        self.assessed.iter().map(|a| &a.change)
    }

    /// How many of the program's entity tables the live schema holds.
    pub fn present(&self) -> usize {
        self.program
            .schema
            .entities
            .iter()
            .filter(|e| self.live.table(&e.name).is_some())
            .count()
    }
}

/// Read the deployment and the live shape, classify and probe. Read-only.
pub async fn plan(pool: &Pool, program: &Program) -> Result<Plan, String> {
    plan_with(pool, program, &Choices::default(), None).await
}

/// [`plan`], with what the developer stated, and with the program the
/// stored amounts were written under when the database cannot say
/// (`written`: a tenant's database has no registry; see
/// [`ordo_schema::diff::diff_with`]).
pub async fn plan_with(
    pool: &Pool,
    program: &Program,
    choices: &Choices,
    written: Option<&Schema>,
) -> Result<Plan, String> {
    let deployed = deployed_artifact(pool).await;
    let prior = deployed.as_ref().and_then(recompile);
    let prior_doc: Option<serde_json::Value> = deployed
        .as_ref()
        .and_then(|a| serde_json::from_str(&a.artifact).ok());
    let live = migrate::live_shape(pool, LIVE)
        .await
        .map_err(|e| format!("reading the live schema: {e}"))?;
    let changes = ordo_schema::diff_with(
        &live,
        prior.as_ref(),
        prior.as_ref().or(written),
        &program.schema,
    );
    let mut assessed = assess_with(pool, &program.schema, &live, changes, choices).await;

    let mut refusals = Vec::new();
    let mut destructive = 0usize;
    for a in &mut assessed {
        // §5 D3, which only a deployed artifact can raise: a drop of
        // something an op in the CURRENTLY ACTIVE artifact reads. The gate
        // would catch this after the fact as drift; catching it here is the
        // point of having a plan at all.
        a.readers = active_readers(prior_doc.as_ref(), &a.change);
        if !a.readers.is_empty() {
            a.refused.push("drop_read_by_active_op".into());
        }
        if a.change.refusal().is_none() && a.change.class() == Class::D {
            destructive += 1;
        }
        for r in &a.refused {
            let class = r.split(" — ").next().unwrap_or(r);
            refusals.push(format!("{class} on `{}`", a.change.entity()));
        }
    }

    Ok(Plan {
        program: program.clone(),
        deployed,
        prior,
        prior_doc,
        live,
        assessed,
        refusals,
        destructive,
        choices: choices.clone(),
    })
}

/// Classify and probe a change list against a live shape, with no deployed
/// artifact in the picture. What [`plan`] does, minus the registry, for a
/// caller that carries its own prior declaration (`ordo dev`).
pub async fn assess(
    pool: &Pool,
    schema: &Schema,
    live: &LiveShape,
    changes: Vec<Change>,
) -> Vec<Assessed> {
    assess_with(pool, schema, live, changes, &Choices::default()).await
}

/// [`assess`], with what the developer stated.
pub async fn assess_with(
    pool: &Pool,
    schema: &Schema,
    live: &LiveShape,
    changes: Vec<Change>,
    choices: &Choices,
) -> Vec<Assessed> {
    let mut out = Vec::with_capacity(changes.len());
    // Per-entity numbers are read once each: a schema with nine changes to
    // one entity must not count its rows nine times.
    let mut extents: BTreeMap<String, (migrate::Extent, Option<f64>)> = BTreeMap::new();
    for c in changes {
        let mut a = Assessed {
            refused: Vec::new(),
            readers: Vec::new(),
            probe: None,
            extent: None,
            change: c,
        };
        if let Some(class) = a.change.refusal() {
            a.refused.push(class.to_string());
            out.push(a);
            continue;
        }
        if a.change.class() == Class::B {
            a.extent = Some(entity_extent(pool, a.change.entity(), &mut extents).await);
        }
        if let Change::ChangePlaces { entity, field, .. } = &a.change {
            let choice = choices.places_for(entity, field);
            match places_check(pool, schema, &a.change, choice).await {
                Err(refusal) => a.refused.push(refusal),
                Ok(probed) => {
                    if matches!(probed, Ok(Probe::Dirty { .. })) {
                        a.refused.push("probe_not_empty".into());
                    }
                    if let Err(e) = &probed {
                        a.refused.push(format!("probe_unavailable — {e}"));
                    }
                    a.probe = Some(probed);
                }
            }
            out.push(a);
            continue;
        }
        let probed = probe(pool, schema, live, &a.change).await;
        match &probed {
            Err(e) => a.refused.push(format!("probe_unavailable — {e}")),
            Ok(Probe::Dirty { .. }) => a.refused.push("probe_not_empty".into()),
            Ok(Probe::Clean(_)) | Ok(Probe::None) => {}
        }
        a.probe = Some(probed.map_err(|e| e.to_string()));
        out.push(a);
    }
    out
}

async fn entity_extent(
    pool: &Pool,
    entity: &str,
    cache: &mut BTreeMap<String, (migrate::Extent, Option<f64>)>,
) -> (migrate::Extent, Option<f64>) {
    if let Some(v) = cache.get(entity) {
        return *v;
    }
    let extent = migrate::extent(pool, entity)
        .await
        .unwrap_or(migrate::Extent { rows: 0, spaces: 0 });
    let rate = migrate::chunk_rate(pool, entity, CHUNK)
        .await
        .ok()
        .flatten();
    cache.insert(entity.to_string(), (extent, rate));
    (extent, rate)
}

/// §5 D3: the ops of the **serving** artifact whose manifest still names
/// what this change drops.
///
/// Read from the deployed artifact document rather than from its recompiled
/// source, because the document's `scope` is the manifest — field-grain,
/// split into `read` and `written`, and the same bytes the grant pinned. A
/// re-derivation could disagree with what is actually deployed, and the
/// whole question here is what is actually deployed.
///
/// Only drops are asked about. An additive change moves an op's contract
/// (that is §5's A-note, counted under GRANTS) but breaks nothing: the
/// column it reads is still there.
pub fn active_readers(prior: Option<&serde_json::Value>, c: &Change) -> Vec<String> {
    let (entity, field) = match c {
        Change::DropField { entity, field } => (entity.as_str(), Some(field.as_str())),
        Change::DropEntity { entity } => (entity.as_str(), None),
        _ => return Vec::new(),
    };
    let Some(prior) = prior else {
        return Vec::new();
    };
    let Some(ops) = prior["ops"].as_array() else {
        return Vec::new();
    };
    let mut out: Vec<String> = ops
        .iter()
        .filter(|o| {
            o["scope"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|s| s["entity"].as_str() == Some(entity))
                .any(|s| match field {
                    // Dropping the entity: naming it at all is enough.
                    None => true,
                    Some(f) => ["read", "written"].iter().any(|half| {
                        s[half]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|v| v.as_str() == Some(f))
                    }),
                })
        })
        .filter_map(|o| o["op"].as_str().map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    out
}

// ── the probes, per class ───────────────────────────────────────────────

/// The C1/C2 answer for a constraint whose column the live table does not
/// have yet — a `unique` arriving on the same deploy as the field it
/// constrains.
///
/// `duplicates` cannot be asked: `GROUP BY` over a column that does not
/// exist is a SQL error, and reporting that as `probe_unavailable` would
/// tell an operator the database was unreachable when in fact the answer is
/// known and is usually **no**.
///
/// It is known because of the null rule. The new column arrives either
/// nullable — every row null — or `NOT NULL DEFAULT <lit>` — every row the
/// same literal; and DL's `null == null` is TRUE, so `ddl_for` emits
/// `NULLS NOT DISTINCT` and both cases collide identically. So a space
/// holding two or more rows violates the constraint the instant it lands,
/// and no probe of the existing data can say otherwise. This is the C1 note
/// stated at its sharpest: the change every other SQL system would accept
/// on a nullable column is the one Ordo cannot.
async fn absent_column_unique(
    pool: &Pool,
    entity: &str,
    what: String,
) -> Result<Probe, ordo_exec::PoolError> {
    let extent = migrate::extent(pool, entity).await?;
    if extent.rows < 2 {
        return Ok(Probe::Clean(format!(
            "{what}: the column arrives with this deploy and `{entity}` holds \
             {} row(s), so nothing can collide",
            extent.rows
        )));
    }
    Ok(Probe::Dirty {
        what: format!("{what}: the column arrives with this deploy"),
        lines: vec![
            format!(
                "`{entity}` holds {} row(s) across {} space(s), and every one of them",
                extent.rows, extent.spaces
            ),
            "gets the SAME value — null for an optional field, the literal for a".into(),
            "defaulted one. DL's `null == null` is TRUE, so the generated DDL is".into(),
            "`UNIQUE NULLS NOT DISTINCT` and at most ONE such row per space is".into(),
            "admissible. Author this as two deploys: add the field, backfill".into(),
            "distinct values, then add the `unique`.".into(),
        ],
    })
}

/// Ask the database the one question the change's class needs answered.
pub async fn probe(
    pool: &Pool,
    schema: &Schema,
    live: &LiveShape,
    c: &Change,
) -> Result<Probe, ordo_exec::PoolError> {
    // Every probe below reads a column. One that the live table does not
    // have yet is not an error and not a clean answer — it is its own
    // question, and asking the database the first one would only produce a
    // syntax error dressed as an outage.
    let live_has = |entity: &str, cols: &[String]| {
        live.table(entity)
            .is_some_and(|t| cols.iter().all(|c| t.columns.contains_key(c)))
    };
    match c {
        // C1/C2 — duplicates, and the null rule that comes with them.
        Change::AddUnique { entity, column, .. } => {
            let what = format!("duplicates over `{entity}`.`{column}`");
            if !live_has(entity, std::slice::from_ref(column)) {
                return absent_column_unique(pool, entity, what).await;
            }
            let found =
                migrate::duplicates(pool, entity, std::slice::from_ref(column), SAMPLE).await?;
            Ok(dup_report(what, found))
        }
        Change::AddCompositeUnique { entity, key, .. } => {
            let cols = resolve_columns(schema, entity, key);
            let what = format!("duplicates over `{entity}` ({})", key.join(", "));
            if !live_has(entity, &cols) {
                return absent_column_unique(pool, entity, what).await;
            }
            let found = migrate::duplicates(pool, entity, &cols, SAMPLE).await?;
            Ok(dup_report(what, found))
        }
        // C3 — values outside the set.
        Change::AddEnum {
            entity,
            field,
            values,
            ..
        } => {
            let what = format!("values outside the set on `{entity}`.`{field}`");
            if !live_has(entity, std::slice::from_ref(field)) {
                return Ok(Probe::Clean(format!(
                    "{what}: the column arrives with this deploy, so it holds only its \
                     declared default"
                )));
            }
            let found = migrate::out_of_enum(pool, entity, field, values, SAMPLE).await?;
            if found.is_empty() {
                return Ok(Probe::Clean(what));
            }
            Ok(Probe::Dirty {
                what: format!("{what}: {} distinct offending value(s)", found.len()),
                lines: found
                    .iter()
                    .map(|o| {
                        format!(
                            "space `{}`  value {}  {} row(s)",
                            o.space,
                            o.value.as_deref().unwrap_or("NULL"),
                            o.rows
                        )
                    })
                    .collect(),
            })
        }
        // C5 — refs with no target.
        Change::AddForeignKey {
            entity,
            field,
            target,
            ..
        } => {
            let what = format!("dangling refs on `{entity}`.`{field}` → `{target}`");
            if !live_has(entity, std::slice::from_ref(field)) {
                return Ok(Probe::Clean(format!(
                    "{what}: the ref column arrives with this deploy, so it names nothing yet"
                )));
            }
            let found = migrate::dangling_refs(pool, entity, field, target).await?;
            if found.is_empty() {
                return Ok(Probe::Clean(what));
            }
            Ok(Probe::Dirty {
                what,
                lines: found
                    .iter()
                    .map(|d| {
                        format!(
                            "space `{}`  {} row(s) name a target that does not exist in \
                             that space",
                            d.space, d.rows
                        )
                    })
                    .collect(),
            })
        }
        // C9 — rows the invariant would refuse. An empty table has none
        // and is not asked; a check over a column that arrives with this
        // deploy would fail on every existing row (a null ordering
        // predicate is FALSE under the language's table), and the count
        // says so in numbers rather than by construction.
        Change::AddCheck {
            entity,
            source,
            sql,
            ..
        } => {
            let what = format!("rows violating `check {source}` on `{entity}`");
            if migrate::extent(pool, entity).await?.rows == 0 {
                return Ok(Probe::Clean(format!(
                    "{what}: `{entity}` is empty, so no row can violate it"
                )));
            }
            let found = migrate::check_violations(pool, entity, sql).await?;
            if found.is_empty() {
                return Ok(Probe::Clean(what));
            }
            let total: i64 = found.iter().map(|n| n.rows).sum();
            Ok(Probe::Dirty {
                what: format!("{what}: {total} row(s) in {} space(s)", found.len()),
                lines: found
                    .iter()
                    .take(SAMPLE)
                    .map(|n| format!("space `{}`  {} row(s)", n.space, n.rows))
                    .collect(),
            })
        }
        // B2/B5 — a required column over rows that may hold null.
        Change::RequireField { entity, field } | Change::AddRequiredField { entity, field } => {
            let what = format!("nulls in `{entity}`.`{field}`");
            // B2 with the column absent: there is no value to count, and no
            // value to write either. Every existing row needs one and the
            // program supplies none, so the answer is the row count.
            if !live_has(entity, std::slice::from_ref(field)) {
                let extent = migrate::extent(pool, entity).await?;
                if extent.rows == 0 {
                    return Ok(Probe::Clean(format!(
                        "{what}: `{entity}` is empty, so a required field with no default \
                         needs no mapping"
                    )));
                }
                return Ok(Probe::Dirty {
                    what: format!(
                        "{what}: {} existing row(s) in {} space(s) need a value this \
                         program does not declare",
                        extent.rows, extent.spaces
                    ),
                    lines: vec![
                        "§5 B2 — a required field with no default requires a declared".into(),
                        "mapping, and mappings are a v1 gap, so this refuses.".into(),
                    ],
                });
            }
            let found = migrate::nulls(pool, entity, field).await?;
            if found.is_empty() {
                return Ok(Probe::Clean(what));
            }
            let total: i64 = found.iter().map(|n| n.rows).sum();
            Ok(Probe::Dirty {
                what: format!(
                    "{what}: {total} row(s) in {} space(s) hold null and no mapping is \
                     declared (§5 B2/B5 — a mapping is a v1 gap, so this refuses)",
                    found.len()
                ),
                lines: found
                    .iter()
                    .take(SAMPLE)
                    .map(|n| format!("space `{}`  {} row(s)", n.space, n.rows))
                    .collect(),
            })
        }
        // B10 — a normalization change over a column that carries a unique.
        Change::Normalize {
            entity,
            field,
            to: Some(norm),
            ..
        } => {
            let unique = schema
                .entity(entity)
                .and_then(|e| e.field(field))
                .is_some_and(|f| f.unique);
            let what = format!("collisions under `normalize({norm})` on `{entity}`.`{field}`");
            if !unique {
                return Ok(Probe::Clean(format!(
                    "{what}: the column carries no unique, so a merge loses no row"
                )));
            }
            let mapping = match norm.as_str() {
                "lower" => "lower({})",
                // The only other mapping the schema can carry today is
                // none. An unknown one is a refusal rather than a guess:
                // probing with the wrong expression answers a different
                // question and looks like an answer to this one.
                _ => {
                    return Ok(Probe::Dirty {
                        what: format!("`{norm}` is not a normalization this probe can express"),
                        lines: vec![
                            "the probe must apply the SAME mapping the backfill will".into()
                        ],
                    })
                }
            };
            let found = migrate::collisions(pool, entity, field, mapping, SAMPLE).await?;
            Ok(dup_report(what, found))
        }
        // B7/B8 — a sealed transform. The epochs must be checked before a
        // plan is offered, and a plan holds no key custody.
        Change::SealField { entity, field } | Change::IndexSealedField { entity, field } => {
            let epochs = migrate::sealed_epochs(pool, entity, field).await.ok();
            let mut lines = vec![
                "every row in every space must be opened, transformed and RE-SEALED under"
                    .to_string(),
                "the current epoch, one key lease per space, inside one freeze window (§7.3)"
                    .to_string(),
            ];
            if let Some(e) = &epochs {
                for x in e.iter().take(SAMPLE) {
                    lines.push(format!(
                        "space `{}`  epoch {}  {} row(s)",
                        x.space, x.epoch, x.rows
                    ));
                }
            }
            lines.push(
                "a row under a DESTROYED epoch cannot be transformed, and writing null in"
                    .to_string(),
            );
            lines.push(
                "its place would be the platform deciding to lose data — so epoch liveness"
                    .to_string(),
            );
            lines.push(
                "must be checked against custody, which a read-only plan does not hold".to_string(),
            );
            Ok(Probe::Dirty {
                what: "sealed_epochs_unverified".into(),
                lines,
            })
        }
        // B3 between `text` and `json` (28b): every stored text must be a
        // JSON document before the column can become `jsonb`.
        Change::ChangeType {
            entity,
            field,
            live,
            declared,
        } if json_conversion(live, declared) == Some(JsonConversion::TextToJson) => {
            let what = format!("`{entity}`.`{field}` values that are not JSON");
            let Some(f) = schema.entity(entity).and_then(|e| e.field(field)) else {
                return Ok(Probe::None);
            };
            let t = ordo_schema::ddl::q(entity);
            let col = ordo_schema::ddl::q(&f.column());
            let rows = pool
                .rows(&format!(
                    "SELECT id FROM {t} WHERE {col} IS NOT NULL \
                     AND NOT pg_input_is_valid({col}, 'jsonb') ORDER BY id"
                ))
                .await?;
            if rows.is_empty() {
                return Ok(Probe::Clean(format!(
                    "{what}: none, so every stored value parses as a document"
                )));
            }
            let mut lines: Vec<String> = rows
                .iter()
                .take(SAMPLE)
                .map(|r| format!("row `{}`", r[0].clone().unwrap_or_default()))
                .collect();
            lines.push("rewrite these as JSON first, or keep the field `text`".into());
            Ok(Probe::Dirty {
                what: format!("{what}: {} row(s)", rows.len()),
                lines,
            })
        }
        Change::ChangeType { live, declared, .. }
            if json_conversion(live, declared) == Some(JsonConversion::JsonToText) =>
        {
            Ok(Probe::Clean(
                "a document becomes its JSON text, which every document has".into(),
            ))
        }
        _ => Ok(Probe::None),
    }
}

// ── a change between `text` and `json` (28b) ─────────────────────────────

/// Which way a B3 type change crosses between `text` and `json`, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonConversion {
    TextToJson,
    JsonToText,
}

/// The two B3 type changes this increment applies in place (28b). Every
/// other type change stays `not_class_a`.
///
/// Both are conversions Postgres does exactly: `jsonb::text` prints a
/// document, and `text::jsonb` parses one, which the probe has proved
/// every stored value is. A sealed field never reaches here: its column is
/// `bytea` either way, so the diff sees no change (see the plan's open
/// question on sealed conversions).
fn json_conversion(live: &str, declared: &str) -> Option<JsonConversion> {
    let base = |t: &str| t.split('(').next().unwrap_or(t).trim().to_string();
    match (base(live).as_str(), base(declared).as_str()) {
        ("text", "jsonb") => Some(JsonConversion::TextToJson),
        ("jsonb", "text") => Some(JsonConversion::JsonToText),
        _ => None,
    }
}

/// The statements that convert one column between `text` and `jsonb`.
///
/// Under an exclusive lock, so no write lands between the guard and the
/// conversion. The guard re-asks the probe's question over rows written
/// since the plan, and refuses rather than lets the cast fail half-way.
/// `rev` moves on every row, as a rescale does, so a client that caches
/// by `rev` refetches the value in its new shape.
fn json_conversion_steps(
    e: &ordo_schema::Entity,
    field: &str,
    way: JsonConversion,
) -> Result<Vec<String>, String> {
    let f = e
        .field(field)
        .ok_or_else(|| format!("`{}.{field}` is not declared", e.name))?;
    let t = ordo_schema::ddl::q(&e.name);
    let col = ordo_schema::ddl::q(&f.column());
    let mut out = vec![format!("LOCK TABLE {t} IN EXCLUSIVE MODE;\n")];
    match way {
        JsonConversion::TextToJson => {
            let why = format!(
                "json_conversion_not_json: a row of {}.{field} written since the plan is not \
                 a JSON document",
                e.name
            );
            out.push(format!(
                "DO $ordo$ BEGIN IF EXISTS (SELECT 1 FROM {t} WHERE {col} IS NOT NULL AND NOT \
                 pg_input_is_valid({col}, 'jsonb')) THEN RAISE EXCEPTION {}; END IF; END \
                 $ordo$;\n",
                ordo_exec::sql_lit(&why)
            ));
            out.push(format!(
                "ALTER TABLE {t} ALTER COLUMN {col} TYPE jsonb USING {col}::jsonb;\n"
            ));
        }
        JsonConversion::JsonToText => {
            out.push(format!(
                "ALTER TABLE {t} ALTER COLUMN {col} TYPE text USING {col}::text;\n"
            ));
        }
    }
    out.push(format!("UPDATE {t} SET \"rev\" = \"rev\" + 1;\n"));
    Ok(out)
}

// ── a change of places (25aa) ───────────────────────────────────────────

/// `10^n` for the places between two declarations, or `None` past what a
/// `bigint` can multiply by.
fn power_of_ten(from: u8, to: u8) -> Option<i64> {
    10i64.checked_pow(u32::from(from.abs_diff(to)))
}

/// `units` minor units at `places`, written as an amount: `2999` at 2 is
/// `29.99`.
fn amount(units: i64, places: u8) -> String {
    if places == 0 {
        return units.to_string();
    }
    let s = format!("{:0>width$}", units, width = usize::from(places) + 1);
    let (whole, frac) = s.split_at(s.len() - usize::from(places));
    format!("{whole}.{frac}")
}

/// The refusal for a change of places that says nothing about the stored
/// amounts: the entity, the field, the old and new places, and the two
/// choices, each with what it does to one amount.
pub fn places_unstated(c: &Change) -> String {
    let Change::ChangePlaces {
        entity,
        field,
        money,
        from,
        to,
    } = c
    else {
        return String::new();
    };
    let target = format!("{entity}.{field}");
    if *money {
        let factor = power_of_ten(*from, *to).unwrap_or(i64::MAX);
        let (verb, dir) = if to > from {
            ("multiplies", "smaller")
        } else {
            ("divides", "larger")
        };
        let example: i64 = 2999 * power_of_ten(0, *from).unwrap_or(1);
        let rescaled = if to > from {
            example.saturating_mul(factor)
        } else {
            example / factor
        };
        format!(
            "places_choice_required — `{target}` goes from money({from}) to money({to}). Its \
             stored amounts are whole numbers of minor units, so the change must say what \
             happens to them. `--places {target}=rescale` {verb} every stored amount by \
             {factor} so each keeps its value ({} stays {}, stored as {rescaled}). \
             `--places {target}=keep` leaves the stored numbers as written, so each reads \
             {factor} times {dir} ({} would read {}).",
            amount(example, *from),
            amount(rescaled, *to),
            amount(example, *from),
            amount(example, *to),
        )
    } else {
        format!(
            "places_choice_required — `{target}` goes from decimal({from}) to decimal({to}). \
             State `--places {target}=rescale` or `--places {target}=keep`. A decimal column \
             holds each value itself, so both keep 29.99 as 29.99 and alter the column to \
             {to} places. Fewer places are refused while any stored value has more."
        )
    }
}

/// Decide a change of places against its stated choice and the rows it
/// would move. `Err` is a refusal by class; `Ok` is the probe.
async fn places_check(
    pool: &Pool,
    schema: &Schema,
    c: &Change,
    choice: Option<PlacesChoice>,
) -> Result<Result<Probe, String>, String> {
    let Change::ChangePlaces {
        entity,
        field,
        money,
        from,
        to,
    } = c
    else {
        return Ok(Ok(Probe::None));
    };
    let Some(choice) = choice else {
        return Err(places_unstated(c));
    };
    let Some(f) = schema.entity(entity).and_then(|e| e.field(field)) else {
        return Err(format!("internal — `{entity}.{field}` is not declared"));
    };
    let Some(factor) = power_of_ten(*from, *to) else {
        return Err(format!(
            "places_too_far — `{entity}.{field}` moves {} places, past what a bigint can scale",
            from.abs_diff(*to)
        ));
    };
    let target = format!("`{entity}`.`{field}`");
    let col = ordo_schema::ddl::q(&f.column());
    let t = ordo_schema::ddl::q(entity);
    if f.sealed && ((*money && choice == PlacesChoice::Rescale) || (!*money && to < from)) {
        return Err(format!(
            "places_sealed — `{entity}.{field}` is sealed, so its stored amounts are \
             ciphertext that SQL cannot {}. That needs the reseal worker, which is not built.{}",
            if *money {
                "rescale"
            } else {
                "check for places"
            },
            if *money {
                format!(" `--places {entity}.{field}=keep` applies with no rewrite.")
            } else {
                String::new()
            }
        ));
    }
    let (what, cond) = match (*money, choice, to > from) {
        (true, PlacesChoice::Keep, _) => {
            return Ok(Ok(Probe::Clean(format!(
                "{target}: keep stated; the stored numbers stay as written and read {factor} \
                 times {}",
                if to > from { "smaller" } else { "larger" }
            ))))
        }
        (true, PlacesChoice::Rescale, true) => (
            format!("amounts on {target} too large to multiply by {factor} in a bigint"),
            format!("abs({col}) > {}", i64::MAX / factor),
        ),
        (true, PlacesChoice::Rescale, false) => (
            format!("amounts on {target} that dividing by {factor} would cut short"),
            format!("{col} % {factor} <> 0"),
        ),
        (false, _, true) => {
            return Ok(Ok(Probe::Clean(format!(
                "{target}: more places keep every stored value"
            ))))
        }
        (false, _, false) => (
            format!("values on {target} with more than {to} places"),
            format!("{col} <> round({col}, {to})"),
        ),
    };
    let rows = pool
        .rows(&format!(
            "SELECT \"id\", {col}::text, count(*) OVER () FROM {t} WHERE {cond} \
             ORDER BY \"id\" LIMIT {SAMPLE}"
        ))
        .await;
    Ok(match rows {
        Err(e) => Err(e.to_string()),
        Ok(rows) if rows.is_empty() => Ok(Probe::Clean(what)),
        Ok(rows) => {
            let g = |r: &Vec<Option<String>>, i: usize| r.get(i).cloned().flatten();
            let total = rows.first().and_then(|r| g(r, 2)).unwrap_or_default();
            let mut lines: Vec<String> = rows
                .iter()
                .map(|r| {
                    format!(
                        "row `{}`  stored {}",
                        g(r, 0).unwrap_or_default(),
                        g(r, 1).unwrap_or_else(|| "NULL".into())
                    )
                })
                .collect();
            lines.push(format!("{total} row(s) in all"));
            Ok(Probe::Dirty { what, lines })
        }
    })
}

/// The statements a stated change of places sends, in the apply's
/// transaction: the rewrite, guarded against rows written since the probe,
/// then the `$places` row that records it. The row and the amounts commit
/// together, so a later plan never proposes the same rescale twice.
fn places_steps(
    c: &Change,
    e: &ordo_schema::Entity,
    choice: PlacesChoice,
    version: &str,
) -> Result<Vec<String>, String> {
    let Change::ChangePlaces {
        field,
        money,
        from,
        to,
        ..
    } = c
    else {
        return Ok(Vec::new());
    };
    let f = e
        .field(field)
        .ok_or_else(|| format!("`{}.{field}` is not declared", e.name))?;
    let factor = power_of_ten(*from, *to)
        .ok_or_else(|| format!("`{}.{field}` moves too many places", e.name))?;
    let t = ordo_schema::ddl::q(&e.name);
    let col = ordo_schema::ddl::q(&f.column());
    let guard = |cond: String, why: String| {
        format!(
            "DO $ordo$ BEGIN IF EXISTS (SELECT 1 FROM {t} WHERE {cond}) THEN RAISE EXCEPTION \
             {}; END IF; END $ordo$;\n",
            ordo_exec::sql_lit(&why)
        )
    };
    let lock = format!("LOCK TABLE {t} IN EXCLUSIVE MODE;\n");
    let mut out = vec![migrate::PLACES_DDL.to_string()];
    // A rescale moves `rev` on every row it rewrites, so a sync pull or a
    // device copy keyed on `rev` refetches the amount in its new places
    // (audit 2026-10-05d, finding 3). It appends no group. A revert past a
    // rescale reads the `$places` row below (its factor and the chain heads
    // it records) to convert the old amount and to expect the moved `rev`
    // (25ad, `ordo_exec::undo`).
    match (*money, choice, to > from) {
        (true, PlacesChoice::Keep, _) => {}
        (true, PlacesChoice::Rescale, true) => {
            out.push(lock);
            out.push(format!(
                "UPDATE {t} SET {col} = {col} * {factor}, \"rev\" = \"rev\" + 1;\n"
            ));
        }
        (true, PlacesChoice::Rescale, false) => {
            out.push(lock);
            out.push(guard(
                format!("{col} % {factor} <> 0"),
                format!(
                    "places_rescale_loses_digits: a row of {}.{field} written since the plan \
                     does not divide by {factor}",
                    e.name
                ),
            ));
            out.push(format!(
                "UPDATE {t} SET {col} = {col} / {factor}, \"rev\" = \"rev\" + 1;\n"
            ));
        }
        // A sealed decimal's ciphertext holds the value as written; more
        // places need no rewrite, and fewer were refused by the plan.
        (false, _, _) if f.sealed => {}
        (false, _, widening) => {
            if !widening {
                out.push(lock);
                out.push(guard(
                    format!("{col} <> round({col}, {to})"),
                    format!(
                        "places_narrowing_loses_digits: a row of {}.{field} written since the \
                         plan has more than {to} places",
                        e.name
                    ),
                ));
            }
            out.push(format!(
                "ALTER TABLE {t} ALTER COLUMN {col} TYPE numeric(1000, {to});\n"
            ));
        }
    }
    out.push(migrate::places_insert(
        &e.name,
        field,
        *from,
        *to,
        *money,
        choice.as_str(),
        version,
        now_ms(),
    ));
    Ok(out)
}

fn dup_report(what: String, found: Vec<migrate::Duplicate>) -> Probe {
    if found.is_empty() {
        return Probe::Clean(what);
    }
    let mut lines: Vec<String> = found
        .iter()
        .map(|d| {
            format!(
                "space `{}`  value {}  {} row(s)",
                d.space,
                d.value.as_deref().unwrap_or("NULL"),
                d.rows
            )
        })
        .collect();
    // §5's C1 note, in the words it asks for. DL's `null == null` is TRUE,
    // so `ddl_for` emits `UNIQUE NULLS NOT DISTINCT` and a nullable unique
    // column admits at most ONE null row per space. This is the line that
    // stops an operator concluding the tool is broken.
    if found.iter().any(|d| d.value.is_none()) {
        lines.push("DL's `null == null` is TRUE, so the generated DDL is `UNIQUE NULLS NOT".into());
        lines.push(
            "DISTINCT` and at most ONE null row per space is admissible. Every other SQL".into(),
        );
        lines.push("system would accept this table.".into());
    }
    Probe::Dirty { what, lines }
}

/// A composite key names FIELDS; the constraint lands on COLUMNS, and for a
/// sealed field those differ. `ddl_for` refuses a composite key over a
/// sealed field outright, so this only has to be right for the plain case —
/// but it is written from the same rule rather than assuming it.
fn resolve_columns(schema: &Schema, entity: &str, key: &[String]) -> Vec<String> {
    let Some(e) = schema.entity(entity) else {
        return key.to_vec();
    };
    key.iter()
        .map(|name| match e.field(name) {
            Some(f) if f.indexed => f.index_column(),
            Some(f) => f.column(),
            None => name.clone(),
        })
        .collect()
}

// ── the target's state ──────────────────────────────────────────────────

/// What the live database holds of a program's schema, against what the
/// registry says is serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaState {
    /// None of the program's entity tables exist and nothing is serving:
    /// a first apply creates the schema.
    Empty,
    /// The registry names a serving version and none of its tables exist.
    /// Not an empty target: creating over it would leave a registry that
    /// describes a database that never existed.
    Ghost { serving: String },
    /// The live schema holds everything this program asserts. `pending`
    /// counts the changes that remain and sequence artifact-first (drops),
    /// which the gate tolerates and increment 5 contracts.
    Matches { pending: usize },
    /// The live schema lacks something this program asserts; the gate
    /// would refuse an artifact of it. The blocking changes.
    Behind(Vec<Change>),
}

/// Read the state. Read-only.
pub async fn schema_state(pool: &Pool, schema: &Schema) -> Result<SchemaState, String> {
    let live = migrate::live_shape(pool, LIVE)
        .await
        .map_err(|e| format!("reading the live schema: {e}"))?;
    let deployed = deployed_artifact(pool).await;
    let present = schema
        .entities
        .iter()
        .filter(|e| live.table(&e.name).is_some())
        .count();
    if present == 0 {
        return Ok(match deployed {
            Some(a) => SchemaState::Ghost { serving: a.version },
            None => SchemaState::Empty,
        });
    }
    let prior = deployed.as_ref().and_then(recompile);
    let changes = ordo_schema::diff(&live, prior.as_ref(), schema);
    let (blocking, pending): (Vec<Change>, Vec<Change>) =
        changes.into_iter().partition(Change::gate_blocking);
    Ok(if blocking.is_empty() {
        SchemaState::Matches {
            pending: pending.len(),
        }
    } else {
        SchemaState::Behind(blocking)
    })
}

// ── the first apply ─────────────────────────────────────────────────────

/// What a first apply did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub tables: usize,
    /// The `$migrations` row.
    pub migration: i64,
    /// Whether the security event landed, in one line.
    pub event: String,
}

/// Create the whole schema on an empty target: the one DDL path the
/// platform already had for a fresh database
/// (`ordo_exec::spacedb::apply_schema`, which `provision::prepare_target`
/// runs for a space's own database), recorded as a `create` migration.
///
/// The caller has established [`SchemaState::Empty`]; this does not
/// re-check it, because `apply_schema`'s own guard (the presence of
/// `members`) answers a narrower question, and a `false` from it here
/// means the two disagreed, which is reported rather than smoothed over.
pub async fn create_schema(
    pool: &Pool,
    program: &Program,
    actor: &str,
    op: &str,
) -> Result<Created, String> {
    let changes = program
        .schema
        .entities
        .iter()
        .map(|e| format!("A1 — new entity `{}`", e.name))
        .collect::<Vec<_>>()
        .join("\n");
    match ordo_exec::spacedb::apply_schema(pool, &program.ddl).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(
                "`members` already exists, so the schema was not created; the target is \
                 not empty in the way `apply_schema` measures it"
                    .into(),
            )
        }
        Err(e) => {
            let _ = record(
                pool,
                &Record {
                    kind: migrate::kind::CREATE,
                    from: None,
                    to: &program.version,
                    actor,
                    op,
                    changes: &changes,
                    ddl: &program.ddl,
                    phase: migrate::phase::FAILED,
                    note: &e.to_string(),
                },
            )
            .await;
            return Err(format!("creating the schema: {e}"));
        }
    }
    let (migration, event) = record(
        pool,
        &Record {
            kind: migrate::kind::CREATE,
            from: None,
            to: &program.version,
            actor,
            op,
            changes: &changes,
            ddl: &program.ddl,
            phase: migrate::phase::APPLIED,
            note: "",
        },
    )
    .await?;
    Ok(Created {
        tables: program.schema.entities.len(),
        migration,
        event,
    })
}

// ── the additive apply ──────────────────────────────────────────────────

/// Where a statement runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// `CREATE TABLE`, first, so everything after may reference it.
    Table,
    /// `ALTER TABLE … ADD`: columns and constraints, in the same
    /// transaction as the tables. A constraint onto a table that exists
    /// is added `NOT VALID` here and validated in [`Phase::Validate`].
    Alter,
    /// `ALTER TABLE … VALIDATE CONSTRAINT`: after the transaction commits,
    /// one statement on its own, so the rows are scanned under a lock that
    /// lets writes continue instead of the `ACCESS EXCLUSIVE` the
    /// transaction held.
    Validate,
    /// `CREATE INDEX CONCURRENTLY`: after the transaction commits, one
    /// statement at a time, because it cannot run inside one.
    ConcurrentIndex,
}

/// One statement the apply will send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub code: &'static str,
    pub phase: Phase,
    pub sql: String,
    /// The index a `ConcurrentIndex` step creates, read back for validity.
    pub index: Option<String>,
    /// The constraint a `Validate` step validates, dropped again when the
    /// validation fails ([`ExecFailure::Validate`]).
    pub constraint: Option<String>,
    /// The table the statement takes its lock on, named when the lock
    /// cannot be had ([`ExecFailure::LockTimeout`]).
    pub table: String,
}

/// A change this increment does not apply, and what a caller can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub change: Change,
    /// A greppable class: `plan_refused`, `not_class_a`, `artifact_first`,
    /// `blind_index_backfill_required`, `constraint_over_existing_rows`.
    pub class: &'static str,
    pub why: String,
    /// The statement this increment would have sent, for an operator who
    /// has read the probe to run by hand. `None` when hand SQL would be
    /// guessing at a transform.
    pub run: Option<String>,
}

/// What the triage decided: every step to send, or every reason not to.
/// Either `refused` is empty and `steps` is the whole apply, or nothing is
/// sent. An apply that landed half its changes would leave a database
/// that matches no version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Triaged {
    pub steps: Vec<Step>,
    pub refused: Vec<Refused>,
}

/// Decide, for every assessed change, the statements to send or the
/// reason not to. Reads row counts; writes nothing.
pub async fn triage(
    pool: &Pool,
    schema: &Schema,
    live: &LiveShape,
    assessed: &[Assessed],
) -> Result<Triaged, String> {
    triage_with(pool, schema, live, assessed, &Choices::default(), "").await
}

/// [`triage`], with what the developer stated and the version being
/// applied, which a change of places records in `$places`.
pub async fn triage_with(
    pool: &Pool,
    schema: &Schema,
    live: &LiveShape,
    assessed: &[Assessed],
    choices: &Choices,
    version: &str,
) -> Result<Triaged, String> {
    // The columns this apply brings into existence, `(entity, column)`.
    // A constraint over only these constrains no existing data.
    let mut arriving: BTreeSet<(String, String)> = BTreeSet::new();
    for a in assessed {
        match &a.change {
            Change::AddEntity { entity } => {
                if let Some(e) = schema.entity(entity) {
                    for pc in ordo_schema::ddl::physical_columns(e) {
                        arriving.insert((entity.clone(), pc.name));
                    }
                }
            }
            Change::AddOptionalField { entity, field }
            | Change::AddConstantField { entity, field, .. } => {
                if let Some(f) = schema.entity(entity).and_then(|e| e.field(field)) {
                    arriving.insert((entity.clone(), f.column()));
                    if f.indexed {
                        arriving.insert((entity.clone(), f.index_column()));
                    }
                }
            }
            _ => {}
        }
    }
    let mut rows: BTreeMap<String, i64> = BTreeMap::new();
    let mut steps = Vec::new();
    let mut refused = Vec::new();
    let ddl_err = |x: ordo_schema::SchemaError| format!("{}: {}", x.code, x.message);

    for a in assessed {
        let c = &a.change;
        if !a.refused.is_empty() {
            refused.push(Refused {
                change: c.clone(),
                class: "plan_refused",
                why: a.refused.join("; "),
                run: None,
            });
            continue;
        }
        let Some(e) = schema.entity(c.entity()) else {
            // A drop names an entity the program no longer declares. It is
            // refused by class; nothing here needs its declaration.
            refused.push(not_built(c));
            continue;
        };
        // Rows in the live table, read once per entity. A table this apply
        // creates holds none.
        let live_rows = if live.table(&e.name).is_some() {
            match rows.get(&e.name) {
                Some(n) => *n,
                None => {
                    let n = migrate::extent(pool, &e.name)
                        .await
                        .map_err(|x| format!("counting `{}`: {x}", e.name))?
                        .rows;
                    rows.insert(e.name.clone(), n);
                    n
                }
            }
        } else {
            0
        };
        let alter = |sql: String| Step {
            code: c.code(),
            phase: Phase::Alter,
            sql,
            index: None,
            constraint: None,
            table: e.name.clone(),
        };
        match c {
            Change::AddEntity { .. } => {
                // A brand-new table: its constraints ride inline, in the
                // transaction. Nothing exists to scan or to lock out.
                steps.push(Step {
                    code: c.code(),
                    phase: Phase::Table,
                    sql: ordo_schema::ddl::create_table(e).map_err(ddl_err)?,
                    index: None,
                    constraint: None,
                    table: e.name.clone(),
                });
                for f in &e.fields {
                    steps.extend(ordo_schema::ddl::foreign_key(e, f).map(alter));
                    steps.extend(ordo_schema::ddl::unique(e, f).map(alter));
                }
                for key in &e.unique_keys {
                    steps.extend(
                        ordo_schema::ddl::composite_unique(e, key)
                            .map_err(ddl_err)?
                            .map(alter),
                    );
                }
                // Plain `CREATE INDEX`, inside the transaction: the table
                // is new and empty, so there is no write to avoid blocking
                // and no invalid-index window to surface.
                for key in &e.all_indexes() {
                    steps.extend(
                        ordo_schema::ddl::index(e, key, false)
                            .map_err(ddl_err)?
                            .map(alter),
                    );
                }
            }
            Change::AddOptionalField { field, .. } | Change::AddConstantField { field, .. } => {
                let Some(f) = e.field(field) else {
                    return Err(format!("`{}.{field}` is not declared", e.name));
                };
                if f.indexed && live_rows > 0 {
                    refused.push(Refused {
                        change: c.clone(),
                        class: "blind_index_backfill_required",
                        why: format!(
                            "`{}` is `bytea NOT NULL` with no default (a null HMACs the null \
                             sentinel), so it cannot land on the {live_rows} row(s) `{}` \
                             holds without a digest for every one of them under every \
                             space's key (§7.3). Increment 4 builds that backfill.",
                            f.index_column(),
                            e.name
                        ),
                        run: None,
                    });
                    continue;
                }
                steps.extend(
                    ordo_schema::ddl::add_column(e, f)
                        .map_err(ddl_err)?
                        .into_iter()
                        .map(alter),
                );
            }
            Change::AddIndex { object, .. } => {
                let key = e
                    .all_indexes()
                    .into_iter()
                    .find(|k| ordo_schema::diff::index_name(&e.name, k) == *object)
                    .ok_or_else(|| format!("`{object}` is not an index `{}` declares", e.name))?;
                if let Some(sql) = ordo_schema::ddl::index(e, &key, true).map_err(ddl_err)? {
                    steps.push(Step {
                        code: c.code(),
                        phase: Phase::ConcurrentIndex,
                        sql,
                        index: Some(object.clone()),
                        constraint: None,
                        table: e.name.clone(),
                    });
                }
            }
            Change::DropIndex { object, .. } => refused.push(Refused {
                change: c.clone(),
                class: "artifact_first",
                why: "the SERVING artifact still declares this index, and the gate compares \
                      index definitions: dropping it now makes the serving artifact drift \
                      on its next load (§8.1). Activate the artifact that no longer declares \
                      it, then drop it."
                    .into(),
                run: Some(format!(
                    "DROP INDEX CONCURRENTLY {};",
                    ordo_schema::ddl::q(object)
                )),
            }),
            Change::AddUnique { field, column, .. } => {
                let Some(f) = e.field(field) else {
                    return Err(format!("`{}.{field}` is not declared", e.name));
                };
                let sql = ordo_schema::ddl::unique(e, f).unwrap_or_default();
                constraint(
                    c,
                    e,
                    std::slice::from_ref(column),
                    live_rows,
                    &arriving,
                    sql,
                    &mut steps,
                    &mut refused,
                );
            }
            Change::AddCompositeUnique { key, .. } => {
                let cols = resolve_columns(schema, &e.name, key);
                let sql = ordo_schema::ddl::composite_unique(e, key)
                    .map_err(ddl_err)?
                    .unwrap_or_default();
                constraint(
                    c,
                    e,
                    &cols,
                    live_rows,
                    &arriving,
                    sql,
                    &mut steps,
                    &mut refused,
                );
            }
            Change::AddEnum { field, object, .. } => {
                let Some(f) = e.field(field) else {
                    return Err(format!("`{}.{field}` is not declared", e.name));
                };
                let clause = ordo_schema::ddl::enum_constraint(e, f).unwrap_or_default();
                let sql = format!(
                    "ALTER TABLE {} ADD {clause};\n",
                    ordo_schema::ddl::q(&e.name)
                );
                steps.extend(not_valid_then_validate(c, e, object, &sql));
            }
            Change::AddForeignKey { field, object, .. } => {
                let Some(f) = e.field(field) else {
                    return Err(format!("`{}.{field}` is not declared", e.name));
                };
                let sql = ordo_schema::ddl::foreign_key(e, f).unwrap_or_default();
                steps.extend(not_valid_then_validate(c, e, object, &sql));
            }
            Change::ChangeType {
                field,
                live,
                declared,
                ..
            } if json_conversion(live, declared).is_some() => {
                let way = json_conversion(live, declared).expect("matched just above");
                steps.extend(json_conversion_steps(e, field, way)?.into_iter().map(alter));
            }
            Change::ChangePlaces { field, .. } => match choices.places_for(&e.name, field) {
                None => refused.push(Refused {
                    change: c.clone(),
                    class: "places_choice_required",
                    why: places_unstated(c),
                    run: None,
                }),
                Some(choice) => {
                    steps.extend(places_steps(c, e, choice, version)?.into_iter().map(alter));
                }
            },
            Change::AddCheck { object, .. } => {
                let Some(chk) = e.checks.iter().find(|k| k.name == *object) else {
                    return Err(format!("`{object}` is not a check `{}` declares", e.name));
                };
                let clause = ordo_schema::ddl::check_constraint(chk);
                let sql = format!(
                    "ALTER TABLE {} ADD {clause};\n",
                    ordo_schema::ddl::q(&e.name)
                );
                steps.extend(not_valid_then_validate(c, e, object, &sql));
            }
            _ => refused.push(not_built(c)),
        }
    }
    steps.sort_by_key(|s| s.phase);
    Ok(Triaged { steps, refused })
}

/// The two steps a foreign key, an enum pin or a check takes onto a table
/// that exists (§3.7): `ADD CONSTRAINT … NOT VALID` in the transaction,
/// a moment's lock and no scan, then `VALIDATE CONSTRAINT` on its own,
/// scanning the rows under a lock that lets writes continue.
///
/// The change reaches the triage only for a table the live shape holds: a
/// constraint on a new table rides inside its `CREATE TABLE`
/// ([`Change::AddEntity`]), so the row count does not enter here. An
/// empty table pays an empty scan.
fn not_valid_then_validate(
    c: &Change,
    e: &ordo_schema::Entity,
    object: &str,
    sql: &str,
) -> [Step; 2] {
    let table = ordo_schema::ddl::q(&e.name);
    let name = ordo_schema::ddl::q(object);
    [
        Step {
            code: c.code(),
            phase: Phase::Alter,
            sql: format!("{} NOT VALID;\n", sql.trim_end().trim_end_matches(';')),
            index: None,
            constraint: None,
            table: e.name.clone(),
        },
        Step {
            code: c.code(),
            phase: Phase::Validate,
            sql: format!("ALTER TABLE {table} VALIDATE CONSTRAINT {name};\n"),
            index: None,
            constraint: Some(object.to_string()),
            table: e.name.clone(),
        },
    ]
}

/// The class-C rule for a unique: apply when it constrains no existing
/// data, refuse with the statement otherwise.
///
/// A unique has no `NOT VALID` form. Its index is built under the lock,
/// so over rows it is refused with the plain statement carried, until
/// §3.7's online form (an index built `CONCURRENTLY`, then
/// `ADD CONSTRAINT … USING INDEX`) is built.
#[allow(clippy::too_many_arguments)]
fn constraint(
    c: &Change,
    e: &ordo_schema::Entity,
    columns: &[String],
    live_rows: i64,
    arriving: &BTreeSet<(String, String)>,
    sql: String,
    steps: &mut Vec<Step>,
    refused: &mut Vec<Refused>,
) {
    let free = live_rows == 0
        || columns
            .iter()
            .all(|col| arriving.contains(&(e.name.clone(), col.clone())));
    if free {
        steps.push(Step {
            code: c.code(),
            phase: Phase::Alter,
            sql,
            index: None,
            constraint: None,
            table: e.name.clone(),
        });
        return;
    }
    refused.push(Refused {
        change: c.clone(),
        class: "constraint_over_existing_rows",
        why: format!(
            "`{}` holds {live_rows} row(s) and this unique names a column they already have. \
             A unique has no `NOT VALID` form, so it would scan every row under an ACCESS \
             EXCLUSIVE lock; its online form (§3.7) is not built yet. The probe is clean, so \
             the statement will succeed, and it is carried for the deployment owner who has \
             read the probe rather than run unasked.",
            e.name
        ),
        run: Some(sql.trim_end().to_string()),
    });
}

fn not_built(c: &Change) -> Refused {
    let why = match c.class() {
        Class::B => {
            "transforming: needs the write barrier, the chunked backfill and the verify pass \
             of increment 3 (sealed and blind-index forms: increment 4). Hand-written SQL \
             would lose the audit spine's record of it."
        }
        Class::C => {
            "a constraint drop is a one-way door (§5 C6): rows that violate the old rule \
             accumulate the instant it goes. It is also artifact-first, because the serving \
             artifact still declares it. Increment 5 owns drops."
        }
        Class::D => {
            "destructive: its own deploy and its own confirmation, with the undo-horizon \
             disclosure (§5 D, §7.2). Increment 5."
        }
        Class::A => "not applied by this increment",
    };
    Refused {
        change: c.clone(),
        class: "not_class_a",
        why: why.into(),
        run: None,
    }
}

/// What executing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executed {
    /// Statements committed in the one transaction.
    pub transactional: usize,
    /// Constraints added `NOT VALID` in it and validated after it.
    pub validated: usize,
    /// Indexes created concurrently after it, all valid.
    pub indexes: usize,
    /// How many times a statement waited out its lock and the apply
    /// backed off and tried again before this succeeded.
    pub retries: u32,
}

/// `ORDO_APPLY_LOCK_TIMEOUT_MS` when unset: how long one statement waits
/// for its lock. `plan-evolution-plans.md` §3.7's default.
pub const LOCK_TIMEOUT_MS: u64 = 2_000;
/// `ORDO_APPLY_LOCK_RETRIES` when unset: how many more times a statement
/// that waited out its lock is tried.
pub const LOCK_RETRIES: u32 = 5;
/// `ORDO_APPLY_LOCK_BACKOFF_MS` when unset: the pause before the first
/// retry. It doubles each retry after that.
pub const LOCK_BACKOFF_MS: u64 = 500;

/// How long a DDL statement waits for its lock, and how the apply tries
/// again when it does not get it (`plan-evolution-plans.md` §3.7).
///
/// A schema statement takes an `ACCESS EXCLUSIVE` lock on its table, and
/// without `lock_timeout` it queues behind every open transaction on that
/// table while every new one queues behind it: a long report holds the
/// apply, and the apply holds the site. With it, the statement gives up
/// after `lock_timeout`, the transaction rolls back, nothing has changed,
/// and the apply waits `backoff` (doubling) and tries again, `retries`
/// times. Then it fails naming the table it could not lock.
///
/// Settings with shipped defaults, read from `ORDO_APPLY_LOCK_TIMEOUT_MS`,
/// `ORDO_APPLY_LOCK_RETRIES` and `ORDO_APPLY_LOCK_BACKOFF_MS`
/// (`docs/settings.md` §7). With the defaults the longest an apply waits
/// is about 27 s: 6 attempts of 2 s and 15.5 s of backoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockPolicy {
    pub lock_timeout: Duration,
    pub retries: u32,
    pub backoff: Duration,
}

impl Default for LockPolicy {
    fn default() -> Self {
        LockPolicy {
            lock_timeout: Duration::from_millis(LOCK_TIMEOUT_MS),
            retries: LOCK_RETRIES,
            backoff: Duration::from_millis(LOCK_BACKOFF_MS),
        }
    }
}

impl LockPolicy {
    /// The environment's values, the defaults for what it leaves unset.
    ///
    /// A value that is not a number reads as unset here, which is the
    /// library's half of the bargain `ordo_exec::PoolLimits::from_env`
    /// strikes: the server's `RuntimeConfig::from_env` refuses the boot on
    /// the same variable, so a deployment never runs on a default it did
    /// not choose, and a tool or test that sets nothing still works.
    pub fn from_env() -> LockPolicy {
        let var = |k: &str| std::env::var(k).ok();
        LockPolicy::parse(
            var("ORDO_APPLY_LOCK_TIMEOUT_MS").as_deref(),
            var("ORDO_APPLY_LOCK_RETRIES").as_deref(),
            var("ORDO_APPLY_LOCK_BACKOFF_MS").as_deref(),
        )
    }

    /// [`LockPolicy::from_env`] over given values, so the reading is
    /// testable without touching the process environment.
    pub fn parse(
        timeout_ms: Option<&str>,
        retries: Option<&str>,
        backoff_ms: Option<&str>,
    ) -> LockPolicy {
        let num = |v: Option<&str>| v.and_then(|s| s.trim().parse::<u64>().ok());
        let d = LockPolicy::default();
        LockPolicy {
            lock_timeout: num(timeout_ms).map_or(d.lock_timeout, Duration::from_millis),
            retries: num(retries)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(d.retries),
            backoff: num(backoff_ms).map_or(d.backoff, Duration::from_millis),
        }
    }

    /// The pause before retry number `retry` (from 1): `backoff` doubled
    /// `retry - 1` times.
    fn pause(&self, retry: u32) -> Duration {
        self.backoff
            .saturating_mul(2u32.saturating_pow(retry.saturating_sub(1).min(16)))
    }

    /// `SET [LOCAL] lock_timeout`, as Postgres spells the value.
    fn set(&self, local: bool) -> String {
        format!(
            "SET {}lock_timeout = '{}ms'",
            if local { "LOCAL " } else { "" },
            self.lock_timeout.as_millis()
        )
    }
}

/// Why executing stopped, and what state it left.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecFailure {
    /// The transaction rolled back. Nothing changed.
    Transaction { sql: String, error: String },
    /// A statement waited [`LockPolicy::lock_timeout`] for a lock on
    /// `table` that another session holds, `attempts` times with backoff
    /// between, and never got it. Nothing changed: each attempt's
    /// transaction rolled back, and an index attempt dropped what it had
    /// begun.
    LockTimeout {
        table: String,
        sql: String,
        attempts: u32,
        lock_timeout: Duration,
        error: String,
    },
    /// The transaction committed with `constraint` added `NOT VALID`, and
    /// `VALIDATE CONSTRAINT` on `table` then failed: a row written between
    /// the plan's probe and the add breaks it, or (`lock_attempts` set) the
    /// table's lock never came. The constraint was dropped again, so the
    /// table holds what the transaction committed and nothing of the
    /// constraint, and the next apply plans it afresh; `dropped` says
    /// whether that drop went through. No index after it was built.
    // The four texts are `Box<str>` to keep `ExecFailure` under clippy's
    // `result_large_err` threshold, which CI's newer toolchain enforces.
    Validate {
        table: Box<str>,
        constraint: Box<str>,
        sql: Box<str>,
        error: Box<str>,
        /// `Some(n)`: the validate waited out its lock `n` times and never
        /// got it. `None`: the rows failed it.
        lock_attempts: Option<u32>,
        transactional: usize,
        validated_before: usize,
        dropped: Result<(), String>,
    },
    /// The transaction committed and a concurrent index after it did not
    /// become valid. Columns and constraints are live; this index is
    /// present by name and used by nothing, and must be dropped before a
    /// retry can create it.
    Index {
        name: String,
        error: Option<String>,
        transactional: usize,
        indexes_before: usize,
    },
}

impl std::fmt::Display for ExecFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecFailure::Transaction { sql, error } => write!(
                f,
                "the transaction rolled back on `{}`: {error}",
                sql.trim()
            ),
            ExecFailure::LockTimeout {
                table,
                sql,
                attempts,
                lock_timeout,
                error,
            } => write!(
                f,
                "could not lock table `{table}`: another session holds a lock on it; gave up \
                 after {attempts} attempt{} of {} ms each (ORDO_APPLY_LOCK_TIMEOUT_MS, \
                 ORDO_APPLY_LOCK_RETRIES). Nothing changed. Statement: `{}`. Last error: {error}",
                if *attempts == 1 { "" } else { "s" },
                lock_timeout.as_millis(),
                sql.trim()
            ),
            ExecFailure::Validate {
                table,
                constraint,
                error,
                lock_attempts,
                dropped,
                ..
            } => {
                match lock_attempts {
                    Some(attempts) => write!(
                        f,
                        "could not lock table `{table}` to validate constraint `{constraint}`: \
                         another session holds a lock on it; gave up after {attempts} \
                         attempt{} (ORDO_APPLY_LOCK_TIMEOUT_MS, ORDO_APPLY_LOCK_RETRIES). \
                         Last error: {error}.",
                        if *attempts == 1 { "" } else { "s" },
                    )?,
                    None => write!(
                        f,
                        "constraint `{constraint}` on table `{table}` failed validation: \
                         existing rows break it ({error}).",
                    )?,
                }
                match dropped {
                    Ok(()) => write!(
                        f,
                        " The constraint was dropped again; the table's other changes are \
                         live. Fix the rows and apply again."
                    ),
                    Err(e) => write!(
                        f,
                        " Dropping the constraint again failed too ({e}); it is in place NOT \
                         VALID. Run `ALTER TABLE {} DROP CONSTRAINT {}` and apply again.",
                        ordo_schema::ddl::q(table),
                        ordo_schema::ddl::q(constraint)
                    ),
                }
            }
            ExecFailure::Index { name, error, .. } => write!(
                f,
                "index `{name}` is INVALID{}; run `DROP INDEX CONCURRENTLY {}` and apply again",
                error
                    .as_ref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default(),
                ordo_schema::ddl::q(name)
            ),
        }
    }
}

/// Send the steps: the tables and alters in one transaction, then each
/// constraint validation on its own, then each concurrent index on its
/// own, read back for validity. Lock waits are bounded by
/// [`LockPolicy::from_env`]; [`execute_with`] takes the policy.
pub async fn execute(pool: &Pool, steps: &[Step]) -> Result<Executed, ExecFailure> {
    execute_with(pool, steps, &LockPolicy::from_env()).await
}

/// [`execute`] under a given [`LockPolicy`] (§3.7).
///
/// Every statement runs with `lock_timeout` set. The transaction sets it
/// `LOCAL`, so it ends with the transaction; a validate and a concurrent
/// index set it for their session and reset it after, on the one
/// connection ([`Pool::rows_in_session`]). A statement that waits it out
/// is [`ordo_exec::PoolError::LockTimeout`], and on that error alone the
/// apply backs off and tries again: the whole transaction from the top,
/// because the server aborted it; the validate as it stands, because a
/// validate that did not finish changed nothing; the index after dropping
/// what the attempt had begun, because an interrupted `CREATE INDEX
/// CONCURRENTLY` leaves an INVALID index by that name and a retry would
/// then fail on the name. Any other error fails as it always did.
///
/// A validate that fails for good, on its rows or on its lock, drops the
/// constraint again before returning ([`ExecFailure::Validate`]).
pub async fn execute_with(
    pool: &Pool,
    steps: &[Step],
    locks: &LockPolicy,
) -> Result<Executed, ExecFailure> {
    let tx_steps: Vec<&Step> = steps
        .iter()
        .filter(|s| matches!(s.phase, Phase::Table | Phase::Alter))
        .collect();
    let validate_steps: Vec<&Step> = steps
        .iter()
        .filter(|s| s.phase == Phase::Validate)
        .collect();
    let idx_steps: Vec<&Step> = steps
        .iter()
        .filter(|s| s.phase == Phase::ConcurrentIndex)
        .collect();
    let attempts = locks.retries.saturating_add(1);
    let mut retries = 0u32;

    let mut transactional = 0usize;
    if !tx_steps.is_empty() {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match run_transaction(pool, &tx_steps, locks).await {
                Ok(n) => {
                    transactional = n;
                    break;
                }
                Err(Attempt::LockTimeout { .. }) if attempt < attempts => {
                    retries += 1;
                    tokio::time::sleep(locks.pause(attempt)).await;
                }
                Err(Attempt::LockTimeout { step, error }) => {
                    return Err(ExecFailure::LockTimeout {
                        table: step.table.clone(),
                        sql: step.sql.clone(),
                        attempts,
                        lock_timeout: locks.lock_timeout,
                        error,
                    });
                }
                Err(Attempt::Failed(f)) => return Err(f),
            }
        }
    }

    let mut validated = 0usize;
    for s in &validate_steps {
        let constraint = s.constraint.clone().unwrap_or_default();
        let mut attempt = 0u32;
        let (error, lock_attempts) = loop {
            attempt += 1;
            match pool
                .rows_in_session(&[&locks.set(false), &s.sql, "RESET lock_timeout"])
                .await
            {
                Ok(_) => break (None, None),
                // The validate changed nothing; the next attempt starts
                // where this one did.
                Err(PoolError::LockTimeout(_)) if attempt < attempts => {
                    retries += 1;
                    tokio::time::sleep(locks.pause(attempt)).await;
                }
                Err(PoolError::LockTimeout(error)) => {
                    break (Some(error.into_string()), Some(attempts))
                }
                Err(e) => break (Some(e.to_string()), None),
            }
        };
        let Some(error) = error else {
            validated += 1;
            continue;
        };
        // The constraint stays NOT VALID otherwise, and the live shape
        // would read it as present: no later apply would validate it. Drop
        // it, so the next apply plans it afresh and its probe names the
        // rows. The drop's own lock wait is bounded the same way.
        let dropped = pool
            .rows_in_session(&[
                &locks.set(false),
                &format!(
                    "ALTER TABLE {} DROP CONSTRAINT IF EXISTS {}",
                    ordo_schema::ddl::q(&s.table),
                    ordo_schema::ddl::q(&constraint)
                ),
                "RESET lock_timeout",
            ])
            .await
            .map(|_| ())
            .map_err(|e| e.to_string());
        return Err(ExecFailure::Validate {
            table: s.table.as_str().into(),
            constraint: constraint.into(),
            sql: s.sql.as_str().into(),
            error: error.into(),
            lock_attempts,
            transactional,
            validated_before: validated,
            dropped,
        });
    }

    let mut indexes = 0usize;
    for s in &idx_steps {
        let name = s.index.clone().unwrap_or_default();
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let sent = pool
                .rows_in_session(&[&locks.set(false), &s.sql, "RESET lock_timeout"])
                .await;
            if let Err(PoolError::LockTimeout(error)) = &sent {
                // What the interrupted build left behind, if anything, so
                // the next attempt (or the next apply) starts clean. Its
                // own lock wait is bounded the same way; if even the drop
                // cannot get in, the index stays and the failure says so.
                let drop = pool
                    .rows_in_session(&[
                        &locks.set(false),
                        &format!(
                            "DROP INDEX CONCURRENTLY IF EXISTS {}",
                            ordo_schema::ddl::q(&name)
                        ),
                        "RESET lock_timeout",
                    ])
                    .await;
                if let Err(e) = drop {
                    return Err(ExecFailure::Index {
                        name,
                        error: Some(format!(
                            "could not lock table `{}` ({error}), and dropping the index the \
                             attempt began failed too: {e}",
                            s.table
                        )),
                        transactional,
                        indexes_before: indexes,
                    });
                }
                if attempt < attempts {
                    retries += 1;
                    tokio::time::sleep(locks.pause(attempt)).await;
                    continue;
                }
                return Err(ExecFailure::LockTimeout {
                    table: s.table.clone(),
                    sql: s.sql.clone(),
                    attempts,
                    lock_timeout: locks.lock_timeout,
                    error: error.to_string(),
                });
            }
            let valid = migrate::index_valid(pool, LIVE, &name).await;
            match (sent, valid) {
                (Ok(_), Ok(Some(true))) => {
                    indexes += 1;
                    break;
                }
                (sent, valid) => {
                    return Err(ExecFailure::Index {
                        name,
                        error: sent
                            .err()
                            .map(|e| e.to_string())
                            .or_else(|| valid.err().map(|e| e.to_string())),
                        transactional,
                        indexes_before: indexes,
                    });
                }
            }
        }
    }
    Ok(Executed {
        transactional,
        validated,
        indexes,
        retries,
    })
}

/// One attempt at the transactional steps.
enum Attempt<'a> {
    /// `step` waited out `lock_timeout`. The server aborted the
    /// transaction; nothing of this attempt landed.
    LockTimeout { step: &'a Step, error: String },
    /// Anything else: the apply's failure as it stands.
    Failed(ExecFailure),
}

async fn run_transaction<'a>(
    pool: &Pool,
    steps: &[&'a Step],
    locks: &LockPolicy,
) -> Result<usize, Attempt<'a>> {
    let begin = |sql: &str, e: PoolError| {
        Attempt::Failed(ExecFailure::Transaction {
            sql: sql.into(),
            error: e.to_string(),
        })
    };
    let tx = pool.begin().await.map_err(|e| begin("BEGIN", e))?;
    let set = locks.set(true);
    tx.rows(&set).await.map_err(|e| begin(&set, e))?;
    let mut n = 0usize;
    for s in steps {
        match tx.rows(&s.sql).await {
            Ok(_) => n += 1,
            // Dropped without commit on both arms: the server aborts it.
            Err(PoolError::LockTimeout(error)) => {
                return Err(Attempt::LockTimeout {
                    step: s,
                    error: error.into_string(),
                });
            }
            Err(e) => {
                return Err(Attempt::Failed(ExecFailure::Transaction {
                    sql: s.sql.clone(),
                    error: e.to_string(),
                }));
            }
        }
    }
    tx.commit().await.map_err(|e| begin("COMMIT", e))?;
    Ok(n)
}

// ── the record ──────────────────────────────────────────────────────────

/// What one `$migrations` row and its event say.
pub struct Record<'a> {
    pub kind: &'a str,
    pub from: Option<&'a str>,
    pub to: &'a str,
    pub actor: &'a str,
    /// What the caller was: `ordo apply`, `ordo migrate --apply`, or the
    /// platform op that 18l adds.
    pub op: &'a str,
    pub changes: &'a str,
    pub ddl: &'a str,
    pub phase: &'a str,
    pub note: &'a str,
}

/// Write the `$migrations` row and its deployment-scoped security event.
/// Returns the row's `seq` and a one-line note about the event.
///
/// The event is recorded the way `ordo keys` records a custody action:
/// observed into a throwaway recorder, drained, appended in one
/// transaction. `space: None` routes it to `Scope::Deployment`, because
/// one set of tables holds every space and a schema change is nobody's
/// tenant record (§7.1). The row is written on its own connection, never
/// inside the DDL transaction, so a failed apply still leaves a row
/// saying it failed.
pub async fn record(pool: &Pool, r: &Record<'_>) -> Result<(i64, String), String> {
    migrate::ensure_migrations_table(pool)
        .await
        .map_err(|e| format!("$migrations: {e}"))?;
    let seq = migrate::record_migration(
        pool,
        &migrate::MigrationRecord {
            seq: 0,
            kind: r.kind.into(),
            from_version: r.from.map(str::to_string),
            to_version: r.to.into(),
            actor: r.actor.into(),
            at: now_ms(),
            changes: r.changes.into(),
            ddl: r.ddl.into(),
            phase: r.phase.into(),
            note: r.note.into(),
        },
    )
    .await
    .map_err(|e| format!("$migrations: {e}"))?;

    let recorder = ordo_secevent::Recorder::default();
    recorder.observe(
        ordo_secevent::Observation {
            space: None,
            subject: ordo_secevent::Subject::member(r.actor),
            op: r.op.into(),
            kind: ordo_secevent::EventKind::SchemaMigration {
                migration: format!("{seq}"),
                phase: r.phase.into(),
            },
            at: ordo_commandlog::Timestamp(now_ms()),
            correlation: ordo_secevent::correlation::adopt(&format!("mig_{seq}")),
        },
        ordo_secevent::Relationship::None,
    );
    let entries = recorder.drain_all(ordo_commandlog::Timestamp(now_ms()));
    let event = match pool.begin().await {
        Err(e) => format!("event NOT RECORDED ({e}); the apply still happened"),
        Ok(tx) => {
            let mut failed = None;
            for e in &entries {
                if let Err(err) = tx.append_security_entry(e).await {
                    failed = Some(err.to_string());
                    break;
                }
            }
            match failed {
                Some(err) => format!("event NOT RECORDED ({err}); the apply still happened"),
                None => match tx.commit().await {
                    Ok(()) => "event recorded on the deployment stream".into(),
                    Err(e) => format!("event NOT RECORDED ({e}); the apply still happened"),
                },
            }
        }
    };
    Ok((seq, event))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── the whole apply, as one call ────────────────────────────────────────

/// What an apply did.
#[derive(Debug, Clone)]
pub enum Applied {
    /// The target was empty; the whole schema was created.
    Created(Created),
    /// The live schema already held everything the program declares.
    Nothing,
    /// Class-A statements landed.
    Additive {
        steps: Vec<Step>,
        executed: Executed,
        /// The `$migrations` row and the event note, or why recording
        /// failed (the DDL still landed).
        recorded: Result<(i64, String), String>,
    },
}

/// Why an apply did not happen, or did not finish.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ApplyRefusal {
    /// The registry names a serving version and none of its tables exist.
    Ghost { serving: String },
    /// The plan refused: a ruling, a deploy-order violation, or a
    /// non-empty probe. `Plan::refusals` names each.
    Plan,
    /// The database does not hold what the serving version declares. The
    /// deployment is refusing to load it, and this program's changes are
    /// measured against a baseline that is not there.
    ServingDrifted { serving: String, drift: Vec<Change> },
    /// The triage refused some change; nothing was sent.
    NotApplied(Triaged),
    /// Creating the schema failed.
    CreateFailed(String),
    /// Executing failed. `recorded` says whether the failure was written.
    ///
    /// Boxed: this is the one variant carrying the steps, the failure and
    /// the record of it, and an unboxed 128-byte `Err` would be paid by
    /// every caller of every function in this module that returns one.
    ExecFailed(Box<ExecFailed>),
    /// Something before execution failed: a count, a DDL rendering.
    Internal(String),
}

/// What [`ApplyRefusal::ExecFailed`] carries: the statements that were
/// sent, how execution failed, and whether that failure reached
/// `$migrations`.
#[derive(Debug, Clone)]
pub struct ExecFailed {
    pub steps: Vec<Step>,
    pub failure: ExecFailure,
    pub recorded: Result<(i64, String), String>,
}

/// Plan, then apply. The plan comes back either way, because what was
/// applied or refused is what it says.
pub async fn apply(
    pool: &Pool,
    program: &Program,
    actor: &str,
    op: &str,
) -> (Result<Plan, String>, Result<Applied, ApplyRefusal>) {
    apply_with(pool, program, actor, op, &Choices::default()).await
}

/// [`apply`], with what the developer stated.
pub async fn apply_with(
    pool: &Pool,
    program: &Program,
    actor: &str,
    op: &str,
    choices: &Choices,
) -> (Result<Plan, String>, Result<Applied, ApplyRefusal>) {
    let plan = match self::plan_with(pool, program, choices, None).await {
        Ok(p) => p,
        Err(e) => return (Err(e.clone()), Err(ApplyRefusal::Internal(e))),
    };
    let outcome = apply_planned(pool, &plan, actor, op).await;
    (Ok(plan), outcome)
}

/// The setting that lets `ordo apply` make safe additive changes itself.
pub const SAFE_CHANGES_SETTING: &str = "ORDO_APPLY_SAFE_CHANGES";

/// Whether [`SAFE_CHANGES_SETTING`] is on. On by default. `off`, `0`,
/// `false` and `no` turn it off. A value this does not know reads as on;
/// the boot refuses such a value (`runtime.rs`).
pub fn safe_changes_enabled() -> bool {
    safe_changes_value(std::env::var(SAFE_CHANGES_SETTING).ok().as_deref()).unwrap_or(true)
}

/// Parse the setting. `None` when the text is not a switch.
pub fn safe_changes_value(v: Option<&str>) -> Option<bool> {
    match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("on") | Some("1") | Some("true") | Some("yes") => Some(true),
        Some("off") | Some("0") | Some("false") | Some("no") => Some(false),
        Some(_) => None,
    }
}

/// A change that cannot lose or reject data: class A, or a foreign key
/// that arrives on a table this same plan creates (so it holds no rows).
fn is_safe_change(c: &Change, all: &[&Change]) -> bool {
    match c {
        Change::AddForeignKey { entity, .. } => all
            .iter()
            .any(|o| matches!(o, Change::AddEntity { entity: e } if e == entity)),
        _ => c.class() == Class::A,
    }
}

/// Would [`apply_safe`] make this plan's changes itself, the setting
/// aside? True when the target holds the program's tables, nothing is
/// refused, and every change is safe. `std.deployment.plan` asks the same
/// question [`apply_safe`] does, so a preview and an apply cannot disagree.
pub fn all_safe(plan: &Plan) -> bool {
    let all: Vec<&Change> = plan.changes().collect();
    plan.present() > 0
        && plan.refusals.is_empty()
        && !all.is_empty()
        && all.iter().all(|c| is_safe_change(c, &all))
}

/// What [`apply_safe`] did.
#[derive(Debug)]
pub enum SafeApply {
    /// Nothing was done: the setting is off, or a change needs more than
    /// class A. The caller refuses as it always did.
    NotSafe,
    /// The safe changes landed. The `$migrations` row and event note, and
    /// each change as `(code, text)`.
    Applied {
        changes: Vec<(String, String)>,
        migration: i64,
        event: String,
    },
}

/// Apply the program's changes over a database that is behind, when every
/// one of them is safe (see [`is_safe_change`]) and the setting is on.
/// The same path as `ordo migrate --apply`: plan, triage, execute, record.
pub async fn apply_safe(
    pool: &Pool,
    program: &Program,
    actor: &str,
    op: &str,
) -> Result<SafeApply, String> {
    if !safe_changes_enabled() {
        return Ok(SafeApply::NotSafe);
    }
    let plan = plan_with(pool, program, &Choices::default(), None).await?;
    if !all_safe(&plan) {
        return Ok(SafeApply::NotSafe);
    }
    let all: Vec<&Change> = plan.changes().collect();
    let changes: Vec<(String, String)> = all
        .iter()
        .map(|c| (c.code().to_string(), c.to_string()))
        .collect();
    match apply_planned(pool, &plan, actor, op).await {
        Ok(Applied::Additive { recorded, .. }) => {
            let (migration, event) = recorded.unwrap_or((0, String::new()));
            Ok(SafeApply::Applied {
                changes,
                migration,
                event,
            })
        }
        Ok(_) => Ok(SafeApply::NotSafe),
        Err(ApplyRefusal::ExecFailed(f)) => Err(format!(
            "the safe schema changes did not finish: {}",
            f.failure
        )),
        Err(ApplyRefusal::Internal(e)) => Err(e),
        Err(_) => Ok(SafeApply::NotSafe),
    }
}

async fn apply_planned(
    pool: &Pool,
    plan: &Plan,
    actor: &str,
    op: &str,
) -> Result<Applied, ApplyRefusal> {
    // The target's state first: a first apply is a create, and a registry
    // over no tables is a refusal, before any change is weighed.
    if plan.present() == 0 {
        if let Some(a) = &plan.deployed {
            return Err(ApplyRefusal::Ghost {
                serving: a.version.clone(),
            });
        }
        return create_schema(pool, &plan.program, actor, op)
            .await
            .map(Applied::Created)
            .map_err(ApplyRefusal::CreateFailed);
    }
    if !plan.refusals.is_empty() {
        return Err(ApplyRefusal::Plan);
    }
    // The serving version's own schema must be what the database holds:
    // every column, constraint and index it asserts, present. Extra live
    // objects are not drift (a staged expand); missing ones are, and a
    // class-A apply over a database the deployment is already refusing to
    // load would make a second problem out of the first.
    if let (Some(prior), Some(a)) = (&plan.prior, &plan.deployed) {
        // A change of places is not drift here: once applied, the amounts
        // are at the new program's places until it is activated, and the
        // load gate is what refuses the serving one meanwhile.
        let drift: Vec<Change> = ordo_schema::diff(&plan.live, None, prior)
            .into_iter()
            .filter(|c| !matches!(c, Change::ChangePlaces { .. }))
            .collect();
        if !drift.is_empty() {
            return Err(ApplyRefusal::ServingDrifted {
                serving: a.version.clone(),
                drift,
            });
        }
    }
    if plan.assessed.is_empty() {
        return Ok(Applied::Nothing);
    }
    let triaged = triage_with(
        pool,
        &plan.program.schema,
        &plan.live,
        &plan.assessed,
        &plan.choices,
        &plan.program.version,
    )
    .await
    .map_err(ApplyRefusal::Internal)?;
    if !triaged.refused.is_empty() {
        return Err(ApplyRefusal::NotApplied(triaged));
    }

    let changes = plan
        .changes()
        .map(|c| match c {
            Change::ChangePlaces { entity, field, .. } => format!(
                "{} — {c} · stated: {}",
                c.code(),
                plan.choices
                    .places_for(entity, field)
                    .map(PlacesChoice::as_str)
                    .unwrap_or("nothing")
            ),
            _ => format!("{} — {c}", c.code()),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let ddl: String = triaged.steps.iter().map(|s| s.sql.as_str()).collect();
    let from = plan.deployed.as_ref().map(|a| a.version.as_str());
    let (outcome, phase, note) = match execute(pool, &triaged.steps).await {
        Ok(executed) => (Ok(executed), migrate::phase::APPLIED, String::new()),
        Err(failure) => {
            let phase = match &failure {
                // A lock that never came: every attempt rolled back, and
                // an index attempt dropped what it had started, so the
                // database is as it was.
                ExecFailure::Transaction { .. } | ExecFailure::LockTimeout { .. } => {
                    migrate::phase::FAILED
                }
                // The transaction committed; the constraint was dropped
                // again, so the table holds the other changes and nothing
                // of it.
                ExecFailure::Validate { .. } => migrate::phase::VALIDATE_FAILED,
                ExecFailure::Index { .. } => migrate::phase::INDEX_INVALID,
            };
            let note = failure.to_string();
            (Err(failure), phase, note)
        }
    };
    let recorded = record(
        pool,
        &Record {
            kind: migrate::kind::ADDITIVE,
            from,
            to: &plan.program.version,
            actor,
            op,
            changes: &changes,
            ddl: &ddl,
            phase,
            note: &note,
        },
    )
    .await;
    match outcome {
        Ok(executed) => Ok(Applied::Additive {
            steps: triaged.steps,
            executed,
            recorded,
        }),
        Err(failure) => Err(ApplyRefusal::ExecFailed(Box::new(ExecFailed {
            steps: triaged.steps,
            failure,
            recorded,
        }))),
    }
}

impl ApplyRefusal {
    /// The greppable class, on `apply_op::ApplyRefused::class`'s
    /// precedent and for its reason: an operator reading a per-target
    /// summary needs one word per target that they can search for, and a
    /// `Debug` dump of a rich enum is not that. This is the lower-level
    /// refusal; the wire-facing one is `apply_op`'s.
    pub fn class(&self) -> &'static str {
        match self {
            ApplyRefusal::Ghost { .. } => "registry_over_no_tables",
            ApplyRefusal::Plan => "plan_refused",
            ApplyRefusal::ServingDrifted { .. } => "serving_schema_drifted",
            ApplyRefusal::NotApplied(_) => "not_class_a",
            ApplyRefusal::CreateFailed(_) => "create_failed",
            ApplyRefusal::ExecFailed(_) => "execute_failed",
            ApplyRefusal::Internal(_) => "internal",
        }
    }
}

// ── bringing one tenant's database to the serving version ───────────────

/// What bringing one database to the serving version did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forward {
    /// The database already records this version. Nothing was written.
    Current,
    /// The database now records this version. `statements` is how many DDL
    /// steps ran to get there: none when it already held everything and only
    /// its record moved, which is what a program that stopped declaring a
    /// field asks of it.
    Advanced { statements: usize },
}

/// Why a database was not brought to the serving version.
#[derive(Debug, Clone)]
pub struct ForwardRefused {
    /// The greppable class: `plan_refused`, `not_class_a`, `unreachable`.
    pub class: &'static str,
    /// What was refused and why, for `/health/ready`, the log and the
    /// apply's report. Never shown to the person calling the tenant.
    pub why: String,
    /// The database could not be asked, or an error came back that says
    /// nothing about the change. Asking again may succeed. A refusal by name
    /// (`not_class_a`, a non-empty probe) says the same thing every time.
    pub transient: bool,
}

impl std::fmt::Display for ForwardRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class, self.why)
    }
}

fn forward_refusal(r: ApplyRefusal, plan: &Plan) -> ForwardRefused {
    let (class, why, transient) = match r {
        ApplyRefusal::Ghost { serving } => (
            "registry_over_no_tables",
            format!("the registry says {serving} is serving and none of its tables exist"),
            false,
        ),
        ApplyRefusal::Plan => (
            "plan_refused",
            plan.refusals.join("; "),
            plan.assessed
                .iter()
                .flat_map(|a| a.refused.iter())
                .any(|r| r.starts_with("probe_unavailable")),
        ),
        ApplyRefusal::ServingDrifted { serving, drift } => (
            "serving_schema_drifted",
            format!(
                "the database lacks {} thing(s) version {serving} declares",
                drift.len()
            ),
            false,
        ),
        ApplyRefusal::NotApplied(t) => (
            "not_class_a",
            t.refused
                .iter()
                .map(|r| format!("{} ({}): {}", r.class, r.change, r.why))
                .collect::<Vec<_>>()
                .join("; "),
            false,
        ),
        ApplyRefusal::CreateFailed(e) => ("create_failed", e, true),
        ApplyRefusal::ExecFailed(f) => ("execute_failed", f.failure.to_string(), false),
        ApplyRefusal::Internal(e) => ("internal", e, true),
    };
    ForwardRefused {
        class,
        why,
        transient,
    }
}

/// **Bring one database, a tenant's, to the version this deployment
/// serves.**
///
/// The same plan and triage the deployment's own apply uses, so a safe
/// additive change lands and anything else is refused by name with nothing
/// sent. Two things differ from [`apply`], and both come from a tenant's
/// database holding no registry to compare against:
///
/// - **A field the program stopped declaring is left where it is.** That is
///   what the deployment's own database does with a drop
///   ([`SchemaState::Matches`]'s `pending`: the gate tolerates the extra
///   column and the contract phase that removes it is not built), so a
///   tenant is treated the same way. Nothing here deletes a column.
/// - **The version is recorded when there was nothing to run.** Without it a
///   database that already holds everything would keep saying it is at the
///   old version and stay closed for ever. It is written only after the live
///   schema is checked to hold everything the version declares.
///
/// A database already at the version is answered without a write, so the
/// walk can be repeated freely.
pub async fn bring_forward(
    pool: &Pool,
    program: &Program,
    actor: &str,
    op: &str,
) -> Result<Forward, ForwardRefused> {
    bring_forward_from(pool, None, program, actor, op).await
}

/// [`bring_forward`], reading what a tenant's database cannot say from the
/// deployment's own (`registry`): the program its stored amounts were
/// written under (the artifact of the version it records), and the places
/// choices stated when the deployment's own database made the same change
/// (`$places`). Without `registry`, a change of places on a tenant's money
/// field is invisible, which is what 25aa found.
pub async fn bring_forward_from(
    pool: &Pool,
    registry: Option<&Pool>,
    program: &Program,
    actor: &str,
    op: &str,
) -> Result<Forward, ForwardRefused> {
    let internal = |e: String| ForwardRefused {
        class: "internal",
        why: e,
        transient: true,
    };
    let before = match ordo_exec::spacedb::recorded_version(pool).await {
        Ok(ordo_exec::spacedb::TargetVersion::At(v)) if v == program.version => {
            return Ok(Forward::Current)
        }
        Ok(ordo_exec::spacedb::TargetVersion::At(v)) => Some(v),
        Ok(ordo_exec::spacedb::TargetVersion::Unknown) => None,
        Err(e) => return Err(internal(e.to_string())),
    };
    let (written, choices) = match registry {
        None => (None, Choices::default()),
        Some(registry) => {
            let written = match &before {
                Some(v) => ordo_exec::registry::get(registry, v)
                    .await
                    .ok()
                    .flatten()
                    .as_ref()
                    .and_then(recompile),
                None => None,
            };
            let stated = migrate::stated_places(registry)
                .await
                .map_err(|e| internal(e.to_string()))?;
            let mut choices = Choices::default();
            for e in &program.schema.entities {
                for f in e.declared() {
                    let Some((_, places)) = ordo_schema::diff::declared_places(f) else {
                        continue;
                    };
                    let key = (e.name.clone(), f.name.clone(), places);
                    if let Some(c) = stated.get(&key).and_then(|c| PlacesChoice::parse(c)) {
                        choices.places.insert((e.name.clone(), f.name.clone()), c);
                    }
                }
            }
            (written, choices)
        }
    };
    let plan = self::plan_with(pool, program, &choices, written.as_ref())
        .await
        .map_err(internal)?;
    let statements = match apply_planned(pool, &plan, actor, op).await {
        Ok(Applied::Additive { steps, .. }) => steps.len(),
        Ok(Applied::Created(c)) => c.tables,
        Ok(Applied::Nothing) => 0,
        Err(r) => return Err(forward_refusal(r, &plan)),
    };
    // What was just applied is checked rather than assumed, because the
    // record written below says the database holds this version.
    match schema_state(pool, &program.schema).await {
        Ok(SchemaState::Matches { .. }) => {}
        Ok(other) => {
            return Err(ForwardRefused {
                class: "still_behind",
                why: format!(
                    "the database still lacks part of this version after the apply: {other:?}"
                ),
                transient: false,
            })
        }
        Err(e) => return Err(internal(e)),
    }
    match ordo_exec::spacedb::recorded_version(pool).await {
        Ok(ordo_exec::spacedb::TargetVersion::At(v)) if v == program.version => {}
        Ok(_) => {
            record(
                pool,
                &Record {
                    kind: migrate::kind::VERSION,
                    from: before.as_deref(),
                    to: &program.version,
                    actor,
                    op,
                    changes: "no change to run: this database already held everything \
                              this version declares",
                    ddl: "",
                    phase: migrate::phase::APPLIED,
                    note: "",
                },
            )
            .await
            .map_err(internal)?;
        }
        Err(e) => return Err(internal(e.to_string())),
    }
    Ok(Forward::Advanced { statements })
}

/// Record that the deployment's own database is at this version when it
/// already held everything the program declares and no row says so.
///
/// The control database's half of [`bring_forward`]'s second rule. A program
/// that only stops declaring a field applies no DDL, so no `$migrations` row
/// moves, and a space routed to this database would read as behind the
/// serving version for ever.
pub async fn record_version_if_behind(
    pool: &Pool,
    program: &Program,
    actor: &str,
    op: &str,
) -> Result<bool, String> {
    let before = match ordo_exec::spacedb::recorded_version(pool)
        .await
        .map_err(|e| e.to_string())?
    {
        ordo_exec::spacedb::TargetVersion::At(v) if v == program.version => return Ok(false),
        ordo_exec::spacedb::TargetVersion::At(v) => Some(v),
        // Version unknown is not behind, and is not written over: a row
        // written now would be a guess about tables some earlier artifact
        // made.
        ordo_exec::spacedb::TargetVersion::Unknown => return Ok(false),
    };
    record(
        pool,
        &Record {
            kind: migrate::kind::VERSION,
            from: before.as_deref(),
            to: &program.version,
            actor,
            op,
            changes: "no change to run: this database already held everything this version \
                      declares",
            ddl: "",
            phase: migrate::phase::APPLIED,
            note: "",
        },
    )
    .await
    .map(|_| true)
}

// ── every target, not just the one we were handed (18d T7, R-S3) ────────

/// One target's outcome from an [`apply_everywhere`] walk.
#[derive(Debug)]
pub struct TargetApply {
    /// Where it is, as [`ordo_exec::route::Target::label`] renders it:
    /// `default`, or `host:port/db`. Never a credential.
    pub label: String,
    /// Whether this database took the program. A target that could not be
    /// **reached** lands here too, as a refusal rather than an absence,
    /// for the reason `targets()` names an unreachable target rather than
    /// leaving it out: a target silently missing from an apply is a tenant
    /// whose database is a version behind and nobody knows.
    pub outcome: Result<Applied, ApplyRefusal>,
    /// **What this target's own `$migrations` said after the apply**, which
    /// is what the fleet view caches (R-S2).
    ///
    /// Read from the target rather than assumed to be the program's
    /// version, because R-S2 makes the target's row the authority and the
    /// control database's row a copy of it. Asserting the program's version
    /// here would make `$fleet` a second claim about the target rather than
    /// a cache of the target's own, and the two could then disagree with
    /// nothing to arbitrate them.
    ///
    /// `None` for a target that was not reached, or whose row could not be
    /// read: the fleet view records observations and there was none.
    pub confirmed: Option<String>,
}

/// What an apply across every database did.
#[derive(Debug)]
pub struct Everywhere {
    /// Per target, **in the order they were applied**: every attached
    /// target first, then the control database.
    pub applied: Vec<TargetApply>,
}

impl Everywhere {
    /// Did every target take it?
    ///
    /// **This is what the activation is gated on** (R-S3). `$artifacts`
    /// and `$activations` are deployment-wide, so a program activated
    /// while one target is behind is a program whose tables that tenant's
    /// database does not have, and every request in that tenant fails per
    /// statement, late, and differently each time.
    pub fn all_succeeded(&self) -> bool {
        self.applied.iter().all(|t| t.outcome.is_ok())
    }

    /// The targets that did not take it, for the caller that has to say so.
    pub fn failures(&self) -> Vec<&TargetApply> {
        self.applied.iter().filter(|t| t.outcome.is_err()).collect()
    }

    /// One line per target, for the operator surface and the event.
    pub fn summary(&self) -> String {
        self.applied
            .iter()
            .map(|t| match &t.outcome {
                Ok(a) => format!("{}: {}", t.label, applied_word(a)),
                Err(e) => format!("{}: refused ({})", t.label, e.class()),
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

fn applied_word(a: &Applied) -> &'static str {
    match a {
        Applied::Created(_) => "created",
        Applied::Nothing => "already held it",
        Applied::Additive { .. } => "applied",
    }
}

/// **Apply a program to every database this deployment serves from**, in
/// the order R-S3 requires.
///
/// # The order, and why the control database is last
///
/// `PoolResolver::targets` lists the default **first**, which is the right
/// order for a reader and the wrong one for an apply. The control database
/// holds `$artifacts`, `$activations` and the fleet's own record, so
/// applying it first would mean the database that records the fleet's
/// state is ahead of the fleet for the duration of the walk. If the
/// process died in that window, recovery would read a control database
/// claiming a version that some tenant's database had never received.
/// Attached targets first, control last, so the record is never ahead of
/// the thing it records.
///
/// The default is identified by its label, which is the contract
/// [`ordo_exec::route::Target`] states in its own doc. A resolver that
/// emits no target by that name is applied in the order it gave, because
/// the alternative is guessing which of its databases is the control one.
///
/// # What a failure does, and does not do
///
/// Nothing is rolled back, and nothing can be: no transaction spans N+1
/// databases, which is the fact this whole design is arranged around. So
/// a target that refuses leaves the targets before it holding the new
/// schema, and the caller's remedy is that **the activation does not
/// land** ([`Everywhere::all_succeeded`]). A deployment in that state is
/// serving the old program against databases that have the new schema,
/// which every class-A change tolerates by construction: additive
/// statements leave the old program's reads working.
///
/// The walk continues past a failure rather than stopping at it. An
/// operator repairing this needs to know whether one target is broken or
/// all of them, and a walk that stopped at the first would make them find
/// that out one deploy at a time.
/// One tenant database's outcome from [`apply_everywhere_except_default`].
#[derive(Debug)]
pub struct TenantApply {
    /// Where it is: `host:port/db`. Never a credential.
    pub label: String,
    pub outcome: Result<Forward, ForwardRefused>,
    /// What the target's own `$migrations` said after the walk (R-S2).
    /// `None` for a target that was not reached or would not answer.
    pub confirmed: Option<String>,
}

/// What the walk over the tenant databases did.
#[derive(Debug, Default)]
pub struct Forwarded {
    pub applied: Vec<TenantApply>,
}

impl Forwarded {
    /// The databases that were not brought forward.
    pub fn failures(&self) -> Vec<&TenantApply> {
        self.applied.iter().filter(|t| t.outcome.is_err()).collect()
    }

    /// The databases that were behind and now are not.
    pub fn advanced(&self) -> usize {
        self.applied
            .iter()
            .filter(|t| matches!(t.outcome, Ok(Forward::Advanced { .. })))
            .count()
    }

    /// The databases that were already at the version.
    pub fn current(&self) -> usize {
        self.applied
            .iter()
            .filter(|t| matches!(t.outcome, Ok(Forward::Current)))
            .count()
    }

    /// Did any database fail in a way that asking again may fix?
    pub fn any_transient(&self) -> bool {
        self.applied
            .iter()
            .any(|t| matches!(&t.outcome, Err(r) if r.transient))
    }
}

/// How many tenant databases one walk works on at once. A slow database
/// holds one of these, not the walk.
pub const FORWARD_CONCURRENCY: usize = 8;

/// How long one database gets before the walk leaves it and moves on. What
/// it did not finish is retried by the catch-up
/// ([`crate::tenant_forward`]).
pub const FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Bring one target forward, bounded, and read back what it says it is at.
pub async fn forward_target(
    target: ordo_exec::route::Target,
    registry: &Pool,
    program: &Program,
    actor: &str,
    op: &str,
) -> TenantApply {
    let outcome = match &target.pool {
        Ok(pool) => {
            match tokio::time::timeout(
                FORWARD_TIMEOUT,
                bring_forward_from(pool, Some(registry), program, actor, op),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => Err(ForwardRefused {
                    class: "timed_out",
                    why: format!(
                        "no answer within {}s, so the walk moved on; it is retried",
                        FORWARD_TIMEOUT.as_secs()
                    ),
                    transient: true,
                }),
            }
        }
        Err(why) => Err(ForwardRefused {
            class: "unreachable",
            why: format!("target `{}` could not be reached: {why}", target.label),
            transient: true,
        }),
    };
    let confirmed = match (&target.pool, outcome.is_ok()) {
        (Ok(pool), true) => match ordo_exec::spacedb::recorded_version(pool).await {
            Ok(ordo_exec::spacedb::TargetVersion::At(v)) => Some(v),
            Ok(ordo_exec::spacedb::TargetVersion::Unknown) => {
                Some(ordo_exec::fleet::UNKNOWN.to_string())
            }
            Err(_) => None,
        },
        _ => None,
    };
    TenantApply {
        label: target.label,
        outcome,
        confirmed,
    }
}

/// **Every target except the control database**, for the caller that has
/// already applied to it.
///
/// `apply_op::sequence` applies the deployment's own database through the
/// schema step, because that is where `$artifacts` and `$activations`
/// live and it has to be reachable before anything else is attempted.
/// This brings the rest to the same version ([`bring_forward`]), a few at a
/// time, each bounded, so one slow or unreachable database costs the walk
/// its own timeout and nothing else. The outcome per database is the
/// answer; nothing is rolled back and nothing here decides whether the
/// activation lands.
///
/// Separate from [`apply_everywhere`] rather than a flag on it: applying
/// the control database twice would write a second `$migrations` row for
/// one apply.
pub async fn apply_everywhere_except_default(
    resolver: &dyn ordo_exec::route::PoolResolver,
    // The deployment's own database: where a tenant's database finds the
    // artifact of the version it records and the places choices stated
    // there ([`bring_forward_from`]).
    registry: &Pool,
    program: &Program,
    actor: &str,
    op: &str,
    // **Proof the caller holds the apply lock** (18d R-S5), not a thing
    // this function uses. A parameter rather than something taken here,
    // for a reason the type then enforces: `ApplyLock` is
    // `pg_advisory_xact_lock` on a dedicated connection, so taking it
    // inside a caller that already holds it would block on itself.
    _lock: &crate::apply_op::ApplyLock,
) -> Result<Forwarded, String> {
    use futures::StreamExt;
    let targets = resolver
        .targets()
        .await
        .map_err(|e| format!("could not list the deployment's databases: {e}"))?;
    let applied = futures::stream::iter(
        targets
            .into_iter()
            .filter(|t| t.label != ordo_exec::route::DEFAULT_LABEL),
    )
    .map(|t| forward_target(t, registry, program, actor, op))
    .buffer_unordered(FORWARD_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;
    Ok(Forwarded { applied })
}

pub async fn apply_everywhere(
    resolver: &dyn ordo_exec::route::PoolResolver,
    program: &Program,
    actor: &str,
    op: &str,
    // **Proof the caller holds the apply lock** (18d R-S5), not a thing
    // this function uses. A parameter rather than something taken here,
    // for a reason the type then enforces: `ApplyLock` is
    // `pg_advisory_xact_lock` on a dedicated connection, so taking it
    // inside a caller that already holds it would block on itself. And
    // `ApplyOps::sequence` must hold it wider than this walk — across the
    // activation too — so it could not live in here even if it were
    // re-entrant. As an argument, no caller can forget it.
    _lock: &crate::apply_op::ApplyLock,
) -> Result<Everywhere, String> {
    let targets = resolver
        .targets()
        .await
        .map_err(|e| format!("could not list the deployment's databases: {e}"))?;

    // Attached first, control last. Stable within each group, so the order
    // is the resolver's beyond the one move this makes.
    let (control, attached): (Vec<_>, Vec<_>) =
        targets.into_iter().partition(|t| t.label == "default");
    let ordered: Vec<ordo_exec::route::Target> = attached.into_iter().chain(control).collect();

    let mut applied = Vec::with_capacity(ordered.len());
    for target in ordered {
        let outcome = match &target.pool {
            // Reached: the ordinary per-target sequence, unchanged. Each
            // target writes its own `$migrations` row, because that row
            // describes that database's shape (R-S2).
            Ok(pool) => self::apply(pool, program, actor, op).await.1,
            // Not reached. An outcome, not a panic.
            Err(why) => Err(ApplyRefusal::Internal(format!(
                "target `{}` could not be reached: {why}",
                target.label
            ))),
        };
        // Observed after the apply, from the target itself. Failure to read
        // it is not a failure of the apply: the DDL either landed or it did
        // not, and that is `outcome`'s business.
        let confirmed = match (&target.pool, outcome.is_ok()) {
            (Ok(pool), true) => match ordo_exec::spacedb::recorded_version(pool).await {
                Ok(ordo_exec::spacedb::TargetVersion::At(v)) => Some(v),
                Ok(ordo_exec::spacedb::TargetVersion::Unknown) => {
                    Some(ordo_exec::fleet::UNKNOWN.to_string())
                }
                Err(_) => None,
            },
            _ => None,
        };
        applied.push(TargetApply {
            label: target.label,
            outcome,
            confirmed,
        });
    }
    Ok(Everywhere { applied })
}
