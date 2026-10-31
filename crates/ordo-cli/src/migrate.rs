//! `ordo plan --against <db>` and `ordo migrate --apply`: the operator's
//! two front doors onto [`ordo_host::schema_apply`].
//!
//! The logic is the service's (owner-ruled 2026-09-20: *"the apply belongs
//! to the service as opposed to a run from the client to the database"*),
//! so everything here is argument parsing, rendering and an exit code. The
//! plan, the probes, the triage, the execution and the record all live in
//! `ordo_host::schema_apply`, which takes a pool and a compiled program and
//! returns structured outcomes; this module prints them. What these two
//! verbs are for is confirming that logic against a real database and
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
//!    non-empty, and for `--apply` the APPLY section: what was sent, or
//!    what was refused by name and what an operator can run instead.
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
use ordo_host::schema_apply::{
    self as sa, Applied, ApplyRefusal, Choices, Phase, Plan, Probe, Program, Triaged,
};
use ordo_schema::diff::{Change, Class};
use ordo_schema::Schema;

use crate::operator::{connect, Flags, Outcome};

/// `ordo plan <program> --against <db>`.
pub(crate) fn run(path: &str, flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--against", "--db", "--places"]) {
        return Outcome::usage(e);
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Outcome::failed(format!("runtime: {e}")),
    };
    rt.block_on(plan_against(path.to_string(), flags))
}

async fn plan_against(path: String, mut flags: Flags) -> Outcome {
    // `--against <db>` names the database; `connect` reads `--db`. One
    // spelling reaching two readers is how they come to disagree, so the
    // value is moved rather than read twice.
    if let Some(db) = flags.named.remove("--against") {
        flags.named.insert("--db".into(), db);
    }
    let src = match crate::Sources::read(std::path::Path::new(&path)) {
        Ok(s) => s,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let db = flags.named.get("--db").cloned().unwrap_or_default();
    let choices = match stated(&flags) {
        Ok(c) => c,
        Err(o) => return o,
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    report(&pool, &src, &path, &db, &choices).await
}

/// `--places <entity>.<field>=rescale|keep[,…]`: what happens to a stored
/// money or decimal field's amounts when its places change (25aa). Refused
/// by the plan until it is stated.
fn stated(flags: &Flags) -> Result<Choices, Outcome> {
    match flags.named.get("--places") {
        None => Ok(Choices::default()),
        Some(spec) => {
            Choices::parse_places(spec).map_err(|e| Outcome::usage(format!("--places {e}")))
        }
    }
}

/// `ordo migrate <program> --apply --db <db> [--as <actor>]`.
///
/// `--apply` is required rather than implied: a verb whose bare form
/// changed a database would be the wrong default for a tool whose other
/// verb is the read-only plan.
pub(crate) fn run_migrate(path: &str, flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--apply", "--db", "--as", "--places"]) {
        return Outcome::usage(e);
    }
    if !flags.switches.iter().any(|s| s == "--apply") {
        return Outcome::usage(
            "migrate needs `--apply`: the read-only form is `ordo plan <program> --against <db>`"
                .into(),
        );
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Outcome::failed(format!("runtime: {e}")),
    };
    rt.block_on(async move {
        let src = match crate::Sources::read(std::path::Path::new(path)) {
            Ok(s) => s,
            Err(e) => return Outcome::failed(format!("{e}\n")),
        };
        let db = flags.named.get("--db").cloned().unwrap_or_default();
        let actor = flags
            .named
            .get("--as")
            .cloned()
            .unwrap_or_else(|| "operator:unattributed".to_string());
        let choices = match stated(&flags) {
            Ok(c) => c,
            Err(o) => return o,
        };
        let pool = match connect(&flags).await {
            Ok(p) => p,
            Err(o) => return o,
        };
        apply_report(&pool, &src, path, &db, &actor, &choices).await
    })
}

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

/// The library entry `ordo migrate --apply` is a shell over. Same contract
/// as [`plan_against_database`], and the same reason for existing.
pub async fn apply_against_database(
    src: &crate::Sources,
    label: &str,
    db: &str,
    actor: &str,
) -> Outcome {
    apply_against_database_with(src, label, db, actor, &Choices::default()).await
}

/// [`apply_against_database`] with `--places` stated.
pub async fn apply_against_database_with(
    src: &crate::Sources,
    label: &str,
    db: &str,
    actor: &str,
    choices: &Choices,
) -> Outcome {
    let mut flags = Flags::default();
    flags.named.insert("--db".into(), db.to_string());
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    apply_report(&pool, src, label, db, actor, choices).await
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
    deployed: Option<&ordo_exec::registry::Artifact>,
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
        "  PLANNABLE  every probe came back empty. `ordo migrate --apply` executes class A\n  \
         (and a constraint that constrains no existing data) and refuses the rest by name;\n  \
         increment 3 is the barrier and the plaintext backfill. Then `ordo apply` stores and\n  \
         activates version {}.\n",
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

// ── `ordo migrate --apply` ───────────────────────────────────────────────

async fn apply_report(
    pool: &Pool,
    src: &crate::Sources,
    path: &str,
    db: &str,
    actor: &str,
    choices: &Choices,
) -> Outcome {
    let program = match compile(src) {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let (plan, outcome) =
        sa::apply_with(pool, &program, actor, "ordo migrate --apply", choices).await;
    let plan = match plan {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let mut r = String::new();
    render_plan(pool, &mut r, &plan, path, db).await;
    let next = format!(
        "  next     `ordo apply {path} --db {}` stores and activates {}\n",
        if db.is_empty() { "…" } else { db },
        plan.program.version
    );
    match outcome {
        Ok(Applied::Created(c)) => {
            r.push_str(&format!(
                "APPLY\n  target   empty: none of this program's tables existed\n  \
                 CREATED  {} table(s) with every constraint and index, in one transaction\n  \
                 APPLIED  recorded as `$migrations` #{}; {}\n{next}",
                c.tables, c.migration, c.event
            ));
            Outcome::ok(r)
        }
        Ok(Applied::Nothing) => {
            r.push_str(
                "APPLY\n  nothing to apply: the live schema already holds everything this \
                 program declares\n",
            );
            r.push_str(&next);
            Outcome::ok(r)
        }
        Ok(Applied::Additive {
            steps,
            executed,
            recorded,
        }) => {
            r.push_str(&format!("APPLY  ({} statement(s))\n", steps.len()));
            render_steps(&mut r, &steps);
            // 27d: a lock another session held was waited out and the
            // apply tried again. Worth a line, because a retry that happens
            // on every deploy is a long transaction somebody should know
            // about.
            let retried = match executed.retries {
                0 => String::new(),
                1 => ", after 1 retry for a lock".to_string(),
                n => format!(", after {n} retries for a lock"),
            };
            r.push_str(&format!(
                "  APPLIED  {} statement(s) in one transaction, {} constraint(s) validated and \
                 {} index(es) built concurrently after it{retried};\n           {}\n",
                executed.transactional,
                executed.validated,
                executed.indexes,
                recorded_line(&recorded)
            ));
            r.push_str(&next);
            Outcome::ok(r)
        }
        Err(ApplyRefusal::Ghost { serving }) => {
            r.push_str(&format!(
                "APPLY\n  REFUSED  serving_schema_drifted — the registry says {serving} is \
                 serving, and none of\n           its tables exist. This is not an empty \
                 target: restore the database, or\n           clear `$artifacts` and \
                 `$activations` if the registry is what is wrong.\n"
            ));
            Outcome::failed(r)
        }
        Err(ApplyRefusal::Plan) => {
            verdict(&mut r, &plan);
            Outcome::failed(r)
        }
        Err(ApplyRefusal::ServingDrifted { serving, drift }) => {
            r.push_str(&format!(
                "APPLY\n  REFUSED  serving_schema_drifted — the database does not hold what the \
                 serving\n           version ({serving}) declares, so the deployment is \
                 refusing to load it, and\n           this program's changes are measured \
                 against a baseline that is not there.\n           Restore or repair the live \
                 schema first; the gap is:\n"
            ));
            for c in &drift {
                r.push_str(&format!("           {:<3} {c}\n", c.code()));
            }
            Outcome::failed(r)
        }
        Err(ApplyRefusal::NotApplied(Triaged { refused, .. })) => {
            r.push_str(&format!(
                "APPLY\n  REFUSED  {} of {} change(s) are not applied by this increment, so \
                 none are: an apply\n           that landed half its changes would leave a \
                 database that matches no version.\n",
                refused.len(),
                plan.assessed.len()
            ));
            for x in &refused {
                r.push_str(&format!(
                    "  {:<3} {}\n       {}\n       {}\n",
                    x.change.code(),
                    x.change,
                    x.class,
                    x.why
                ));
                if let Some(run) = &x.run {
                    r.push_str("       to run by hand, after reading the probe:\n");
                    for line in run.lines() {
                        r.push_str(&format!("         {line}\n"));
                    }
                }
            }
            Outcome::failed(r)
        }
        Err(ApplyRefusal::CreateFailed(e)) => {
            r.push_str(&format!("APPLY\n  FAILED   {e}\n"));
            Outcome::failed(r)
        }
        Err(ApplyRefusal::ExecFailed(failed)) => {
            let sa::ExecFailed {
                steps,
                failure,
                recorded,
            } = *failed;
            r.push_str(&format!("APPLY  ({} statement(s))\n", steps.len()));
            render_steps(&mut r, &steps);
            r.push_str(&format!("  FAILED   {failure}\n"));
            match &failure {
                sa::ExecFailure::Transaction { .. } => {
                    r.push_str("           the transaction rolled back; nothing changed\n")
                }
                sa::ExecFailure::LockTimeout { table, .. } => r.push_str(&format!(
                    "           another session holds a lock on `{table}`; nothing changed. \
                     Wait for it, or end it, and apply again\n"
                )),
                sa::ExecFailure::Validate {
                    transactional,
                    validated_before,
                    dropped,
                    ..
                } => r.push_str(&format!(
                    "           the {transactional} transactional statement(s) are committed \
                     and {validated_before} earlier constraint(s) are validated;\n           {}\n",
                    match dropped {
                        Ok(()) => {
                            "this constraint was dropped again, so the table holds everything \
                             else and nothing of it. Fix the rows it names and apply again"
                        }
                        Err(_) => {
                            "this constraint is in place NOT VALID: new writes are checked, the \
                             rows it names are not. Drop it as the message says, then apply again"
                        }
                    }
                )),
                sa::ExecFailure::Index {
                    transactional,
                    indexes_before,
                    ..
                } => r.push_str(&format!(
                    "           the {transactional} transactional statement(s) are committed \
                     and {indexes_before} earlier index(es) are valid;\n           this index \
                     is present by name and used by nothing until it is dropped and re-created\n"
                )),
                other => r.push_str(&format!("           {other}\n")),
            }
            r.push_str(&format!("           {}\n", recorded_line(&recorded)));
            Outcome::failed(r)
        }
        Err(ApplyRefusal::Internal(e)) => {
            r.push_str(&format!("APPLY\n  FAILED   {e}\n"));
            Outcome::failed(r)
        }
        Err(e) => {
            r.push_str(&format!("APPLY\n  FAILED   {e:?}\n"));
            Outcome::failed(r)
        }
    }
}

fn render_steps(r: &mut String, steps: &[sa::Step]) {
    for s in steps {
        let where_ = match s.phase {
            Phase::Table | Phase::Alter => "tx ",
            Phase::Validate => "vc ",
            Phase::ConcurrentIndex => "cc ",
        };
        for (i, line) in s.sql.trim_end().lines().enumerate() {
            r.push_str(&format!(
                "  {} {:<3} {line}\n",
                if i == 0 { where_ } else { "   " },
                if i == 0 { s.code } else { "" }
            ));
        }
    }
}

fn recorded_line(recorded: &Result<(i64, String), String>) -> String {
    match recorded {
        Ok((seq, event)) => format!("recorded as `$migrations` #{seq}; {event}"),
        Err(e) => format!("NOT RECORDED ({e}); the DDL still landed"),
    }
}
