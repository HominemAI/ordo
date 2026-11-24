//! `ordo plan`'s report of a plan against a database, and the refusal of
//! the removed `ordo migrate`.
//!
//! `ordo migrate --apply --direct` is removed (ruled 2026-11-05: every
//! evolution command is a door op). `ordo evolve` replaces it
//! (`plan-versions.md` increment 8); [`removed`] is what the verb answers.
//! What follows is the plan report's own description.
//!
//! The logic is the service's (owner-ruled 2026-09-20: *"the apply belongs
//! to the service as opposed to a run from the client to the database"*),
//! so everything here is argument parsing, rendering and an exit code. The
//! plan, the probes, the triage, the execution and the record all live in
//! `ordo_host::schema_apply`, which takes a pool and a compiled program and
//! returns structured outcomes; this module prints them. What the report
//! is for is confirming that logic against a real database and
//! testing it; a hosted control plane (checklist 18l) calls the same
//! functions from behind an owner-gated platform op.
//!
//! # What the plan prints, and the order it prints it in
//!
//! 1. **Which program, which database, which version** — including the
//!    version the deployment is serving now, because a plan against an
//!    unknown baseline is a plan against nothing.
//! 2. **The tenancy banner**, first and unconditionally (§6). It says what
//!    the plan was read against, how each tenant's database gets the
//!    change, and whom a transforming migration freezes. A reader who sees
//!    it after the change list has already sized the outage wrong.
//! 3. **The classified change list**, one line per change with its taxonomy
//!    code, then its probe, then its cost.
//! 4. **The verdict**, which is a refusal whenever any probe came back
//!    non-empty.
//!
//! # The refusal rule, and why it is a refusal rather than a warning
//!
//! §5: *"the declaration is not the question; the live data is"*, and a
//! non-empty probe is a **refusal, not a warning**, because which row
//! survives is the deployment's decision. `nfc.rs` settled the same rule
//! for the same reason. So the plan prints no appliable plan when any
//! probe is non-empty, the apply sends nothing, and the exit code is 1 — a
//! cron that reads `!= 0` must not be able to gate a deploy on a report it
//! did not read.
//!
//! # What the plan deliberately cannot tell you
//!
//! - **A rename**, which reaches a live shape as a drop and an add. §5 B4
//!   requires a declared mapping; guessing from a matching type would
//!   propose copying one column into another.
//! - **Whether a class-B change will succeed**, as opposed to whether it is
//!   currently blocked. Increments 3 and 4 execute; this one plans.
//! - **A number from your production hardware.** The freeze estimate is
//!   measured on the database it is pointed at, and it is a floor — see
//!   [`ordo_exec::migrate::chunk_rate`].

use std::collections::BTreeMap;

use ordo_exec::migrate;
use ordo_exec::Pool;
use ordo_host::schema_apply::{self as sa, Choices, Plan, Probe, Program};
use ordo_schema::diff::{Change, Class};
use ordo_schema::Schema;

use crate::operator::{connect, Flags, Outcome};

/// `ordo plan <program> --host <h>`: what an apply of the program would do
/// to the deployment, worked out by the deployment and not done
/// (`std.deployment.plan`). The plan goes through the door (owner, 2026-11-05).
pub(crate) fn run(path: &str, flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--host", "--token", "--space"]) {
        return Outcome::usage(e);
    }
    let host = match crate::wire::requested(&flags) {
        Some(Ok(h)) => h,
        Some(Err(e)) => return Outcome::usage(e),
        None => {
            return Outcome::usage(
                "plan goes through the door: name it with `--host <url>`\n".into(),
            )
        }
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Outcome::failed(format!("runtime: {e}")),
    };
    rt.block_on(async {
        let src = match crate::Sources::read(std::path::Path::new(path)) {
            Ok(s) => s,
            Err(e) => return Outcome::failed(format!("{e}\n")),
        };
        crate::wire::plan_over_the_wire(&host, &flags, src.text(), path).await
    })
}

/// `ordo migrate`, removed (ruled 2026-11-05: every evolution command is a
/// door op; `plan-versions.md` increment 8 replaces it). The verb stays only
/// to refuse, naming what replaces it, so a script that still types it
/// learns the new commands instead of "unknown command".
pub(crate) fn removed() -> Outcome {
    Outcome::usage(format!("{REMOVED}\n"))
}

/// What `ordo migrate` answers.
pub const REMOVED: &str = "`ordo migrate` is removed: every schema evolution goes through the \
     deployment's door (ruled 2026-11-05). Store the new version with `ordo apply <dir> \
     --no-activate --host <h>`, run the step with `ordo evolve --from <version> --to <version> \
     --run --host <h>`, then `ordo apply <dir> --host <h>` activates it. Each step's \
     plan and receipt is listed by `ordo evolve --list`.";

/// The library entry `ordo plan --against <db>` is a shell over.
///
/// Exposed because the command's whole contract is *what it prints and what
/// it exits with*, and a test that shells out to a binary to read that back
/// is testing argv. `label` is what the report calls the program — a path
/// from the CLI, a name from a test.
pub async fn plan_against_database(src: &crate::Sources, label: &str, db: &str) -> Outcome {
    plan_against_database_with(src, label, db, &Choices::default()).await
}

/// [`plan_against_database`] with `--places` stated.
pub async fn plan_against_database_with(
    src: &crate::Sources,
    label: &str,
    db: &str,
    choices: &Choices,
) -> Outcome {
    let mut flags = Flags::default();
    flags.named.insert("--db".into(), db.to_string());
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    report(&pool, src, label, db, choices).await
}

/// Compile through the CLI's own diagnostic path first, so a program that
/// does not compile gets the spanned message `ordo check` would print
/// rather than the gate's one-liner; then through the service's, which is
/// what will be planned and applied.
pub(crate) fn compile(src: &crate::Sources) -> Result<Program, String> {
    crate::load_sources(src)?;
    Program::compile(src.text())
}

// ── the report ───────────────────────────────────────────────────────────

async fn report(
    pool: &Pool,
    src: &crate::Sources,
    path: &str,
    db: &str,
    choices: &Choices,
) -> Outcome {
    let program = match compile(src) {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let plan = match sa::plan_with(pool, &program, choices, None).await {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let mut r = String::new();
    render_plan(pool, &mut r, &plan, path, db).await;
    verdict(&mut r, &plan);
    if plan.refusals.is_empty() {
        Outcome::ok(r)
    } else {
        Outcome::failed(r)
    }
}

/// Everything above the verdict: header, tenancy, changes, grants.
async fn render_plan(pool: &Pool, r: &mut String, plan: &Plan, path: &str, db: &str) {
    header(
        r,
        path,
        db,
        plan.deployed.as_ref(),
        &plan.program.version,
        plan.prior.is_some(),
    );
    tenancy(r, pool, &plan.program.schema, &plan.live).await;
    externals(r, plan);

    if plan.assessed.is_empty() {
        // **Not an early return.** A deploy that changes only op bodies has
        // no schema change and still kills every grant pinning those ops
        // (§5's A-note), so the GRANTS section is exactly the part of this
        // report that matters here. Returning before it printed "nothing to
        // apply" over a re-consent campaign.
        r.push_str("CHANGES  none\n\n");
        grants(pool, r, plan.prior_doc.as_ref(), &plan.program.doc).await;
        return;
    }

    r.push_str(&format!("CHANGES  ({})\n\n", plan.assessed.len()));
    for a in &plan.assessed {
        let c = &a.change;
        r.push_str(&format!("  {:<3} {}\n", c.code(), c));
        if let Some(class) = c.refusal() {
            r.push_str(&format!("      REFUSED  {class} — {}\n\n", why(class)));
            continue;
        }
        match c.class() {
            Class::A => r.push_str("      class A · auto-applies, no barrier, no probe\n"),
            Class::B => {
                let (extent, rate) = a
                    .extent
                    .unwrap_or((migrate::Extent { rows: 0, spaces: 0 }, None));
                r.push_str(&format!(
                    "      class B · WRITE BARRIER on `{}` in this database\n\
                     \x20     freeze   {} over {} space(s), {} row(s)\n\
                     \x20              {}\n",
                    c.entity(),
                    migrate::freeze_estimate(extent.rows, rate),
                    extent.spaces,
                    extent.rows,
                    rate_note(rate),
                ));
            }
            Class::C => r.push_str("      class C · probe first; a non-empty probe is a refusal\n"),
            Class::D => {
                r.push_str(
                    "      class D · destructive: its own deploy and its own confirmation\n",
                );
            }
        }
        if !a.readers.is_empty() {
            r.push_str(&format!(
                "      REFUSED  drop_read_by_active_op — {} op(s) in the SERVING artifact still\n\
                 \x20              name this: {}\n\
                 \x20              Deploy order violation (§5 D3): the artifact that stops \
                 reading it\n\x20              must be activated first, and only then may the \
                 column go.\n",
                a.readers.len(),
                a.readers.join(", ")
            ));
        }
        // A change of places (25aa): stated or not, and why not.
        if let Change::ChangePlaces { entity, field, .. } = c {
            if let Some(choice) = plan.choices.places_for(entity, field) {
                r.push_str(&format!("      stated   {choice}\n"));
            }
            for x in a.refused.iter().filter(|x| x.starts_with("places_")) {
                r.push_str(&format!("      REFUSED  {x}\n"));
            }
        }
        match &a.probe {
            Some(Err(e)) => r.push_str(&format!("      REFUSED  probe_unavailable — {e}\n")),
            Some(Ok(Probe::Clean(what))) => r.push_str(&format!("      probe    {what}: clean\n")),
            Some(Ok(Probe::None)) | None => {}
            Some(Ok(Probe::Dirty { what, lines })) => {
                r.push_str(&format!("      REFUSED  probe_not_empty — {what}\n"));
                for l in lines {
                    r.push_str(&format!("               {l}\n"));
                }
            }
        }
        if let Change::DropField { entity, field } = c {
            undo_disclosure(pool, r, entity, std::slice::from_ref(field)).await;
        }
        if let Change::DropEntity { entity } = c {
            let fields: Vec<String> = plan
                .prior
                .as_ref()
                .and_then(|p| p.entity(entity))
                .map(|e| e.declared().map(|f| f.name.clone()).collect())
                .unwrap_or_default();
            undo_disclosure(pool, r, entity, &fields).await;
        }
        r.push('\n');
    }

    grants(pool, r, plan.prior_doc.as_ref(), &plan.program.doc).await;
}

// ── the sections ─────────────────────────────────────────────────────────

fn header(
    r: &mut String,
    path: &str,
    db: &str,
    deployed: Option<&ordo_host::versions::Artifact>,
    version: &str,
    recompiled: bool,
) {
    r.push_str(&format!(
        "program    {path}\ndatabase   {}\n",
        if db.is_empty() {
            "(from ORDO_PG_DB)"
        } else {
            db
        }
    ));
    match deployed {
        None => r.push_str(
            "serving    nothing  (no artifact registry, or nothing activated — every drop is \
             therefore invisible to this plan; see TENANCY)\n",
        ),
        Some(a) => {
            r.push_str(&format!("serving    {}  (actor {})\n", a.version, a.actor));
            if !recompiled {
                r.push_str(
                    "           NOTE  the serving artifact's source did not recompile, so \
                     drops and\n           declaration-only changes (`normalize`, the typed-id \
                     key) are not classified\n",
                );
            }
        }
    }
    r.push_str(&format!("proposed   {version}\n\n"));
}

async fn tenancy(r: &mut String, pool: &Pool, schema: &Schema, live: &ordo_schema::LiveShape) {
    // The space count is read from whichever declared entity the live
    // database actually has, because that is the only place the answer is
    // stored: there is no table listing a deployment's spaces.
    let mut spaces = 0i64;
    for e in &schema.entities {
        if live.table(&e.name).is_some() {
            if let Ok(x) = migrate::extent(pool, &e.name).await {
                spaces = spaces.max(x.spaces);
            }
        }
    }
    r.push_str(&format!(
        "TENANCY\n  \
         Read against: the database named above, which holds {spaces} space(s). Every\n  \
         tenant has a database of its own, so this plan describes that one database.\n  \
         How each tenant's database gets the change: the tenant catch-up brings it to\n  \
         the version the deployment serves (at boot, when the deployment swaps, and\n  \
         every ORDO_TENANT_CATCHUP_SECS). It applies additive (class A) changes, with\n  \
         any places choice stated on the deployment's own database. Any other change\n  \
         is refused for the tenant by name: that tenant stays closed, with the reason\n  \
         on /health/ready, until it is migrated by hand.\n  \
         Not built: a transforming (class B) migration that walks every tenant. Run\n  \
         it one tenant database at a time, with --db.\n  \
         What a transforming migration freezes: writes to that entity in the database\n  \
         it runs on, so the tenants whose database that is. Other tenants are not\n  \
         frozen, and each one's backfill is timed on its own rows.\n\n"
    ));
}

/// The external entities and what sits beside them (ruled 2026-11-15,
/// `external-entities`): a note for each external with no `access read`
/// (which `shared` chose, and every member of every space reads whole), the
/// columns each translation does not read, and the "present, not Ordo's"
/// list of tables and schemas the database holds that are not Ordo's. Nothing
/// is changed. A table that is gone, changed or unreadable is a refusal in
/// the verdict, not here.
fn externals(r: &mut String, plan: &Plan) {
    let mut lines = external_notes(&plan.program.schema);
    lines.extend(plan.external.display_notes());
    if lines.is_empty() {
        return;
    }
    r.push_str("EXTERNAL\n");
    for n in lines {
        r.push_str(&format!("  {n}\n"));
    }
    r.push('\n');
}

/// One line per external entity that has no `access read`, and one more for
/// each public external (ruled 2026-11-15, `external-entities` ask 5): the
/// public path has no guard walk, so a public op serves every row.
pub fn external_notes(schema: &Schema) -> Vec<String> {
    let mut notes = Vec::new();
    for e in &schema.externals {
        if e.guards.is_none() {
            notes.push(format!(
                "note: `{}` has no `access read`: every member of every space reads every row",
                e.name
            ));
        }
        if e.has_public() {
            let fields: Vec<String> = e
                .declared()
                .filter(|f| f.public)
                .map(|f| format!("`{}`", f.name))
                .collect();
            notes.push(format!(
                "note: `{}` is public: an op declared public may serve its public fields \
                 ({}) to anonymous callers on the public path. Ordo never writes the \
                 table, so a cached answer lives until ORDO_PUBLIC_CDN_S_MAXAGE ends \
                 (default {} seconds)",
                e.name,
                fields.join(", "),
                ordo_api::PublicSettings::default().s_maxage
            ));
        }
    }
    notes
}

fn rate_note(rate: Option<f64>) -> String {
    match rate {
        None => "the scan rate could not be measured on this database".into(),
        Some(v) => format!(
            "measured at {v:.0} rows/s on a read-only chunk scan — a real backfill\n\
             \x20              chunk also pays for the write, the WAL and the command-log \
             append,\n\x20              so this is a FLOOR, not a prediction"
        ),
    }
}

/// §7.2's disclosure: how much of the undo horizon this drop destroys.
async fn undo_disclosure(pool: &Pool, r: &mut String, entity: &str, fields: &[String]) {
    if fields.is_empty() {
        return;
    }
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(
            ordo_exec::cadence::LogPolicy::DEFAULT
                .undo_horizon
                .as_secs(),
        );
    match migrate::undo_horizon_loss(pool, entity, fields, cutoff).await {
        Err(e) => r.push_str(&format!(
            "      undo     could not be counted ({e}) — treat the loss as unknown\n"
        )),
        Ok(loss) if loss.is_empty() => r.push_str(
            "      undo     no retained group inside the undo horizon names these field(s)\n",
        ),
        Ok(loss) => {
            let total: i64 = loss.iter().map(|l| l.groups).sum();
            r.push_str(&format!(
                "      undo     {total} retained group(s) inside the undo horizon become\n\
                 \x20              UNREVERTABLE the instant this drops — `undo.rs` refuses any\n\
                 \x20              revert whose logged image names a field the schema no longer\n\
                 \x20              declares (`unrevertable_image`)\n"
            ));
            for l in loss.iter().take(8) {
                r.push_str(&format!(
                    "               space `{}`  {} of {} group(s)\n",
                    l.space, l.groups, l.examined
                ));
            }
        }
    }
}

/// §5's A-note: this deploy's cost in re-consent.
///
/// An op's manifest carries its artifact `version`
/// (`ordo_host::catalog_view`), and a capability grant pins the combined
/// hash of the manifests it consented to — so a grant dies exactly when
/// some op it names moves version. That makes this an exact count rather
/// than an estimate, and it is computed from the two artifact documents
/// rather than by re-deriving the hash, because the CLI must not depend on
/// the wiring crate that owns it (`ordo_plan::artifact`'s module doc).
async fn grants(
    pool: &Pool,
    r: &mut String,
    prior: Option<&serde_json::Value>,
    now: &serde_json::Value,
) {
    let versions = |doc: &serde_json::Value| -> BTreeMap<String, String> {
        doc["ops"]
            .as_array()
            .map(|ops| {
                ops.iter()
                    .filter_map(|o| {
                        Some((
                            o["op"].as_str()?.to_string(),
                            o["version"].as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let Some(prior) = prior else {
        r.push_str(
            "GRANTS\n  \
             No deployed artifact to compare against, so no manifest can be shown to have\n  \
             moved. Every outstanding grant survives a deploy that changes no manifest;\n  \
             this plan cannot tell you which case you are in.\n\n",
        );
        return;
    };
    let (before, after) = (versions(prior), versions(now));
    let mut moved: Vec<&String> = before
        .iter()
        .filter(|(op, v)| after.get(*op).is_none_or(|w| w != *v))
        .map(|(op, _)| op)
        .collect();
    moved.sort();
    if moved.is_empty() {
        r.push_str(
            "GRANTS\n  \
             0 killed. No op's manifest moves, so every outstanding capability grant\n  \
             survives this deploy.\n\n",
        );
        return;
    }
    r.push_str(&format!(
        "GRANTS\n  \
         {} op(s) change contract, so every grant pinning them dies and needs a human\n  \
         to re-consent:\n",
        moved.len()
    ));
    for op in moved {
        let n = live_grants(pool, op).await;
        match n {
            None => r.push_str(&format!(
                "  {op:<32} (no grant registry in this database)\n"
            )),
            Some(n) => r.push_str(&format!("  {op:<32} {n} grant(s)\n")),
        }
    }
    r.push('\n');
}

async fn live_grants(pool: &Pool, op: &str) -> Option<i64> {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM \"$capabilitygrant\" WHERE \"op\" = {} \
             AND \"revoked_at\" IS NULL",
            ordo_exec::sql_lit(op)
        ))
        .await
        .ok()?;
    rows.first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|s| s.parse().ok())
}

fn verdict(r: &mut String, plan: &Plan) {
    r.push_str("VERDICT\n");
    if !plan.refusals.is_empty() {
        r.push_str(&format!(
            "  REFUSED  {} probe(s)/ruling(s) blocked this plan:\n",
            plan.refusals.len()
        ));
        for x in &plan.refusals {
            r.push_str(&format!("           {x}\n"));
        }
        r.push_str(
            "  No appliable plan is printed. Which row survives is this deployment's\n  \
             decision, and a plan that chose for you would be making it.\n",
        );
        return;
    }
    if plan.assessed.is_empty() {
        r.push_str(
            "  The live schema already holds everything this program declares. No DDL to \
             apply;\n  see GRANTS above for what the artifact swap alone costs.\n",
        );
        return;
    }
    r.push_str(&format!(
        "  PLANNABLE  every probe came back empty. `ordo apply --no-activate` stores version {},\n  \
         `ordo evolve --from <version> --to {0} --run` runs the step through the door, and\n  \
         `ordo apply` activates it.\n",
        plan.program.version
    ));
    if plan.destructive > 0 {
        r.push_str(&format!(
            "  {} destructive change(s) need their own deploy and their own \
             confirmation,\n  and only that deploy is unrecoverable without a restore.\n",
            plan.destructive
        ));
    }
}

/// Why a ruling refuses, in the words the ruling uses.
fn why(class: &str) -> &'static str {
    match class {
        "unseal_refused" => {
            "unsealing moves every value permanently into plaintext columns, WAL and\n\
             \x20              backups, and re-sealing does not undo that (§5 D6). If it is ever\n\
             \x20              allowed it needs its own consent surface, not a migration class."
        }
        "key_change_refused" => {
            "the typed-id key is minted into every id ever issued and stored as text\n\
             \x20              in every ref column that names one (§5 D4). It is a schema fact,\n\
             \x20              fixed so every log line ever emitted stays self-describing."
        }
        _ => "refused by ruling",
    }
}
