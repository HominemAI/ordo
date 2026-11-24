//! **Stored plans** (`plan-versions.md` §2.3, increment 8): `ordo evolve`,
//! the door op `std.deployment.evolve`, `$plans`, the plan digest, the
//! receipt and replay.
//!
//! # What a stored plan is
//!
//! One step from a stored version X to a stored version Y (D5). Its parts:
//!
//! - **The edge**: X and Y, each with its source identity. The stored plans
//!   are the version graph: each one names its parent (X). A tenant on 6
//!   reaches 9 by the stored path 6 to 7, 7 to 8, 8 to 9, never by a diff of
//!   6 against 9 ([`path`]).
//! - **The options**: what the developer chose for each change that needs a
//!   choice. Today that is a change of places (`--places`). Option
//!   expressions are Ordo expressions, never SQL.
//! - **The receipt**: each change, its class, the option chosen, and its
//!   reverse, then the forward statements. A record, never an input
//!   ([`Rendered::receipt`]).
//! - **The digest**: SHA-256 over the receipt's canonical bytes.
//!
//! # Rendering is the same in every environment (D3)
//!
//! [`render`] reads only X's and Y's stored sources and the options. The
//! change list is Y's schema diffed against an **empty database built from
//! X's DDL**, in a shadow namespace of its own, never against the live
//! tables. The statements are the triage's for that empty database
//! ([`crate::schema_apply::triage_counted`] with no rows). So staging,
//! re-rendering what dev rendered, gets the same digest, and a difference
//! refuses ([`EvolveRefusal::DigestMismatch`]).
//!
//! What an instance binds stays out of the digest (review 2026-11-23): an
//! external's resolved schema and its live shape, the live rows, settings.
//! An external entity never appears in a step; its check runs per database
//! at run time, in `plan_with`. The translation in the source is inside,
//! through the source identities.
//!
//! **System SQL is rendered at run time.** A statement on one of Ordo's own
//! tables (`$places`, `$migrations`) is left out of the receipt and the
//! digest ([`is_system_statement`]). Each run renders its step again on the
//! database it runs on, with the stored options, so a stored plan never
//! freezes Ordo's own tables.
//!
//! # A plan with errors is never stored
//!
//! A change the step cannot make (a class B change the menu does not offer
//! yet, a change of places with no choice) refuses the render, naming each
//! one and its fix, and nothing is stored.
//!
//! A removed field or entity is **orphaned**, not an error (ruled
//! 2026-10-01, 27y): no statement, the column is kept, and the reverse is
//! serving it again. Pruning is `ordo evolve --prune` (increment 14).
//!
//! # The stale check runs per database, at run time
//!
//! The plan stores the from-version it expects. [`run_step`] reads what the
//! database records ([`ordo_exec::spacedb::recorded_version`]) and refuses
//! a database at any other version ([`EvolveRefusal::Stale`]), and the
//! apply refuses a database that lacks part of X ([`EvolveRefusal::Drifted`]).
//! Render-time probes are advisory; the per-database probes run in the run.
//!
//! # Pre-checks (increment 9)
//!
//! After a plan is stored, `std.deployment.evolve` runs its pre-checks on
//! every database ([`crate::precheck`]) and lists every issue with its fix.
//! `--run` refuses while any is open ([`EvolveRefusal::PreChecks`]), and
//! [`run_step`] refuses a database whose own data breaks the step the same
//! way. A step that removes a sign-in method some people rely on is not
//! stored until the deployment owner approves that count
//! ([`evolve_approved`], ruled 2026-11-23, Q6).
//!
//! # Readers (`plan-evolution-federation.md` F1)
//!
//! A step that retypes or removes a field of a table on a `reader` line
//! lists every reader of that table, with the earliest date its grant on the
//! old column can end. A warning: it refuses nothing. The date is outside
//! the digest, because it is a fact of the day it was rendered.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ordo_api::{ErrorCode, ExecError};
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_schema::diff::{Change, Class};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::schema_apply::{self as sa, Choices, Program};
use crate::versions::{Artifact, VersionRefusal, Versions};

/// The receipt's format. A release that changes what a receipt holds moves
/// this, and with it every digest, so a plan rendered by the older release
/// refuses rather than running under a different meaning.
pub const RECEIPT_FORMAT: u64 = 1;

/// The soak's default length in days (`soak.length`, §2.2). Soaks are
/// increment 11a; until then the readers' notice dates from this default.
pub const SOAK_DEFAULT_DAYS: u64 = 7;

/// The op string a run writes into `$migrations` and the security stream.
pub const OP: &str = "std.deployment.evolve";

/// `$plans`, beside `$artifacts`: one row per stored plan, append-only. Both
/// ends must be stored versions, so a plan never names a version the
/// deployment does not hold.
pub const PLANS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$plans\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"from_version\" text NOT NULL REFERENCES \"$artifacts\" (\"version\"), \
  \"to_version\" text NOT NULL REFERENCES \"$artifacts\" (\"version\"), \
  \"digest\" text NOT NULL, \
  \"options\" text NOT NULL, \
  \"receipt\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"at\" bigint NOT NULL, \
  UNIQUE (\"from_version\", \"to_version\", \"digest\") \
); \
CREATE INDEX IF NOT EXISTS \"$plans_edge\" ON \"$plans\" (\"from_version\", \"to_version\", \"seq\")";

// ── the typed error ─────────────────────────────────────────────────────

/// One change a render cannot make, and how to make it possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    /// The taxonomy code, `B3`.
    pub code: String,
    /// The change in words.
    pub change: String,
    /// The greppable class from the triage: `not_class_a`,
    /// `places_choice_required`, ...
    pub class: String,
    pub why: String,
    /// What the developer can do.
    pub fix: String,
}

/// Why a plan was not rendered, stored or run. One typed error for every
/// caller (rule 9); [`EvolveRefusal::to_exec_error`] is the one place it
/// becomes a door answer.
#[derive(Debug)]
pub enum EvolveRefusal {
    /// The version store refused: an unknown version, a database fault.
    Version(VersionRefusal),
    /// A stored version's source does not compile under this release.
    Uncompilable { version: String, why: String },
    /// `--from` and `--to` name the same version.
    SameVersion(String),
    /// `--places` could not be read.
    Options(String),
    /// The render found changes it cannot make. Nothing was stored.
    Errors {
        from: String,
        to: String,
        errors: Vec<PlanError>,
    },
    /// The digest the caller expected is not the one this deployment
    /// renders (D3). Nothing was stored or run.
    DigestMismatch {
        from: String,
        to: String,
        expected: String,
        rendered: String,
    },
    /// No stored plan, or no stored path, leads from one version to the
    /// other.
    NoPath { from: String, to: String },
    /// The database records another version than the step expects.
    Stale {
        database: String,
        recorded: Option<String>,
        expected: String,
    },
    /// The database lacks part of the version the step starts from.
    Drifted {
        database: String,
        version: String,
        drift: Vec<String>,
    },
    /// The step was refused on the database, or failed there.
    Run {
        database: String,
        from: String,
        to: String,
        class: &'static str,
        why: String,
        transient: bool,
    },
    /// The version removes a sign-in method some people rely on, and the
    /// deployment owner has not approved it for this count (ruled
    /// 2026-11-23, Q6). Nothing was stored.
    SignInRemoval {
        removal: crate::precheck::SignInRemoval,
        /// The count an approval named, when one was given.
        approved: Option<u64>,
    },
    /// The pre-checks found data the step's new shape would refuse
    /// (27w). Every issue, from every database asked.
    PreChecks {
        from: String,
        to: String,
        issues: Vec<crate::precheck::Issue>,
    },
    /// Another apply holds the deployment.
    Busy,
    /// A database fault outside the version store.
    Store(String),
}

impl From<VersionRefusal> for EvolveRefusal {
    fn from(e: VersionRefusal) -> EvolveRefusal {
        EvolveRefusal::Version(e)
    }
}

impl From<PoolError> for EvolveRefusal {
    fn from(e: PoolError) -> EvolveRefusal {
        EvolveRefusal::Version(VersionRefusal::Store(e))
    }
}

impl EvolveRefusal {
    /// A greppable class.
    pub fn class(&self) -> &'static str {
        match self {
            EvolveRefusal::Version(_) => "version_refused",
            EvolveRefusal::Uncompilable { .. } => "version_uncompilable",
            EvolveRefusal::SameVersion(_) => "same_version",
            EvolveRefusal::Options(_) => "options_invalid",
            EvolveRefusal::Errors { .. } => "plan_has_errors",
            EvolveRefusal::DigestMismatch { .. } => "plan_digest_mismatch",
            EvolveRefusal::NoPath { .. } => "no_stored_path",
            EvolveRefusal::Stale { .. } => "plan_stale",
            EvolveRefusal::Drifted { .. } => "database_drifted",
            EvolveRefusal::Run { class, .. } => class,
            EvolveRefusal::SignInRemoval { .. } => "sign_in_removal_unapproved",
            EvolveRefusal::PreChecks { .. } => "precheck_failed",
            EvolveRefusal::Busy => "apply_in_progress",
            EvolveRefusal::Store(_) => "store_failed",
        }
    }

    /// Asking again unchanged may succeed.
    pub fn transient(&self) -> bool {
        match self {
            EvolveRefusal::Version(e) => {
                matches!(
                    e.class(),
                    ordo_fault::FaultClass::Busy | ordo_fault::FaultClass::Retryable
                )
            }
            EvolveRefusal::Run { transient, .. } => *transient,
            EvolveRefusal::Busy | EvolveRefusal::Store(_) => true,
            _ => false,
        }
    }

    /// **The one place an `EvolveRefusal` becomes a door answer.**
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            EvolveRefusal::Version(e) => e.to_exec_error(what),
            EvolveRefusal::SameVersion(_) | EvolveRefusal::Options(_) => {
                ExecError::new(ErrorCode::InvalidParam, message)
            }
            EvolveRefusal::NoPath { .. } => ExecError::new(ErrorCode::NotFound, message),
            EvolveRefusal::Busy => ExecError::new(ErrorCode::ApplyRefused, message),
            EvolveRefusal::Store(_) => ExecError::of_fault(
                ordo_fault::FaultClass::Broken,
                ErrorCode::SpaceUnavailable,
                message,
            ),
            EvolveRefusal::Run {
                transient: true, ..
            } => ExecError::of_fault(
                ordo_fault::FaultClass::Retryable,
                ErrorCode::SpaceUnavailable,
                message,
            ),
            _ => ExecError::new(ErrorCode::ApplyRefused, message),
        }
    }
}

impl std::fmt::Display for EvolveRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvolveRefusal::Version(e) => write!(f, "{e}"),
            EvolveRefusal::Uncompilable { version, why } => write!(
                f,
                "version_uncompilable: version `{version}`'s stored source does not compile \
                 under this Ordo release: {why}. Run `ordo fix` on its source, or render from \
                 another version"
            ),
            EvolveRefusal::SameVersion(v) => write!(
                f,
                "same_version: `--from` and `--to` both name `{v}`. A plan is one step \
                 between two stored versions"
            ),
            EvolveRefusal::Options(e) => write!(f, "options_invalid: {e}"),
            EvolveRefusal::Errors { from, to, errors } => {
                write!(
                    f,
                    "plan_has_errors: the step from `{from}` to `{to}` has {} change(s) it \
                     cannot make, and nothing was stored:",
                    errors.len()
                )?;
                for e in errors {
                    write!(
                        f,
                        "\n  {} {}: {} ({}). Fix: {}",
                        e.code, e.change, e.why, e.class, e.fix
                    )?;
                }
                Ok(())
            }
            EvolveRefusal::DigestMismatch {
                from,
                to,
                expected,
                rendered,
            } => write!(
                f,
                "plan_digest_mismatch: the step from `{from}` to `{to}` renders here as \
                 {rendered}, and {expected} was expected. The sources, the options or the \
                 Ordo release differ from where it was rendered. Compare the receipts, or \
                 render it again with `ordo evolve --from {from} --to {to}` where it was \
                 rehearsed"
            ),
            EvolveRefusal::NoPath { from, to } => write!(
                f,
                "no_stored_path: no stored plan leads from `{from}` to `{to}`. Render each \
                 missing step with `ordo evolve --from <version> --to <next>`; `ordo evolve \
                 --list` shows the stored ones"
            ),
            EvolveRefusal::Stale {
                database,
                recorded,
                expected,
            } => write!(
                f,
                "plan_stale: database `{database}` records version `{}`, and this step starts \
                 from `{expected}`. Replay the stored path from the version it records with \
                 `ordo evolve --tenant <tenant>`",
                recorded.as_deref().unwrap_or("none")
            ),
            EvolveRefusal::Drifted {
                database,
                version,
                drift,
            } => write!(
                f,
                "database_drifted: database `{database}` lacks {} thing(s) version `{version}` \
                 declares ({}). Bring it to `{version}` first; nothing was run",
                drift.len(),
                drift.join("; ")
            ),
            EvolveRefusal::Run {
                database,
                from,
                to,
                class,
                why,
                ..
            } => write!(
                f,
                "{class}: the step from `{from}` to `{to}` was not run on database \
                 `{database}`: {why}"
            ),
            EvolveRefusal::SignInRemoval { removal, approved } => {
                write!(
                    f,
                    "sign_in_removal_unapproved: version `{}` removes the sign-in method(s) {}, \
                     and {} person(s) would be left with no method. Nothing was stored.",
                    removal.to,
                    removal
                        .removed
                        .iter()
                        .map(|m| format!("`{m}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    removal.people
                )?;
                if let Some(n) = approved {
                    write!(
                        f,
                        " The approval named {n} person(s); the count is now {}.",
                        removal.people
                    )?;
                }
                write!(f, " Migration steps:")?;
                for s in removal.steps() {
                    write!(f, "\n  {s}")?;
                }
                Ok(())
            }
            EvolveRefusal::PreChecks { from, to, issues } => {
                write!(
                    f,
                    "precheck_failed: the step from `{from}` to `{to}` meets {} issue(s) in the \
                     data, and nothing was run. Fix each, then run it again:",
                    issues.len()
                )?;
                for i in issues {
                    write!(f, "\n  {}", i.line())?;
                }
                Ok(())
            }
            EvolveRefusal::Busy => write!(
                f,
                "apply_in_progress: another apply or move holds the deployment. Try again \
                 when it ends"
            ),
            EvolveRefusal::Store(e) => write!(f, "store_failed: {e}"),
        }
    }
}

// ── the options ─────────────────────────────────────────────────────────

/// The options as the plan stores them: `entity.field=choice`, sorted,
/// comma-separated. Empty when there are none. Each fill (10b) follows on a
/// line of its own, `fill <audit> <entity>.<field>=<mapping>`, so a plan
/// with no fill stores exactly what it stored before fills existed.
pub fn options_text(c: &Choices) -> String {
    let mut out = c
        .places
        .iter()
        .map(|((e, f), ch)| format!("{e}.{f}={}", ch.as_str()))
        .collect::<Vec<_>>()
        .join(",");
    for ((e, f), fill) in &c.fills {
        out.push_str(&format!(
            "\nfill {} {e}.{f}={}",
            fill.audit.as_str(),
            fill.mapping.text()
        ));
    }
    out
}

/// Read stored or typed options back.
pub fn parse_options(text: &str) -> Result<Choices, EvolveRefusal> {
    let mut lines = text.split('\n');
    let places = lines.next().unwrap_or_default();
    let mut c = if places.trim().is_empty() {
        Choices::default()
    } else {
        Choices::parse_places(places)
            .map_err(|e| EvolveRefusal::Options(format!("--places {e}")))?
    };
    for line in lines.map(str::trim).filter(|l| !l.is_empty()) {
        let bad = || EvolveRefusal::Options(format!("a stored fill does not read back: `{line}`"));
        let rest = line.strip_prefix("fill ").ok_or_else(bad)?;
        let (audit, spec) = rest.split_once(' ').ok_or_else(bad)?;
        let images = match audit {
            "summarised" => "",
            "images" => spec.split_once('=').map(|(t, _)| t).ok_or_else(bad)?,
            _ => return Err(bad()),
        };
        c.parse_fills(spec, images)
            .map_err(|e| EvolveRefusal::Options(format!("--fill {e}")))?;
    }
    Ok(c)
}

/// The options a caller types: `--places`, `--fill` and `--audit-images`.
pub fn typed_options(places: &str, fill: &str, images: &str) -> Result<Choices, EvolveRefusal> {
    let mut c = parse_options(places)?;
    c.parse_fills(fill, images)
        .map_err(|e| EvolveRefusal::Options(format!("--fill {e}")))?;
    Ok(c)
}

// ── the render ──────────────────────────────────────────────────────────

/// One reader of a table a step changes (F1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderNotice {
    /// The reader's nickname, from the `reader` line.
    pub reader: String,
    pub table: String,
    /// The field, or `None` when the whole entity goes.
    pub field: Option<String>,
    /// The change in words.
    pub change: String,
    /// The earliest the reader's grant on the old column can end: the
    /// render's time plus the soak's default length, unix seconds.
    pub grant_ends_not_before: u64,
}

impl ReaderNotice {
    pub fn line(&self) -> String {
        format!(
            "reader `{}` reads `{}`{}: {}. Its grant on the old column ends no earlier than \
             {} (the soak's close; soaks on a read table close together).",
            self.reader,
            self.table,
            self.field
                .as_deref()
                .map(|f| format!(", field `{f}`"))
                .unwrap_or_default(),
            self.change,
            crate::versions::utc_minute(self.grant_ends_not_before)
        )
    }
}

/// What a render made.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub from: String,
    pub to: String,
    /// The options, as stored.
    pub options: String,
    /// The receipt: what the digest is taken over.
    pub receipt: Value,
    pub digest: String,
    /// F1: the readers of each table the step changes. Outside the digest.
    pub readers: Vec<ReaderNotice>,
    /// `prev_stale`: marks whose rename X already has. Warnings, never
    /// refusals, and outside the digest.
    pub warnings: Vec<String>,
}

/// The digest of a receipt: SHA-256 of its canonical bytes, as
/// `sha256:<hex>`.
pub fn digest_of(receipt: &Value) -> String {
    let bytes = ordo_plan::artifact::to_bytes(receipt);
    let d = Sha256::digest(bytes.as_bytes());
    format!(
        "sha256:{}",
        d.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

/// Whether a rendered statement writes one of Ordo's own tables. Those are
/// rendered at run time on the database the step runs on, never stored.
pub fn is_system_statement(sql: &str) -> bool {
    let s = sql.trim_start();
    s.contains("\"$places\"")
        || s.contains("\"$migrations\"")
        || s.starts_with("CREATE TABLE IF NOT EXISTS \"$")
}

/// A removed field or entity: kept as an orphan, never dropped by a step.
fn orphaned(c: &Change) -> bool {
    matches!(c, Change::DropField { .. } | Change::DropEntity { .. })
}

/// **Render the step from `from` to `to`**, each a stored version's name or
/// `#n`. Reads only the two stored sources and `options`; changes nothing
/// but a shadow namespace it drops again.
pub async fn render(
    pool: &Pool,
    from: &str,
    to: &str,
    options: &Choices,
    at: u64,
) -> Result<Rendered, EvolveRefusal> {
    let v = Versions::on(pool);
    let from = v.resolve(from).await?;
    let to = v.resolve(to).await?;
    if from == to {
        return Err(EvolveRefusal::SameVersion(from));
    }
    let (Some(a), Some(b)) = (v.get(&from).await?, v.get(&to).await?) else {
        return Err(VersionRefusal::UnknownVersion(from).into());
    };
    render_stored(pool, &a, &b, options, at).await
}

pub(crate) fn compiled(a: &Artifact) -> Result<Program, EvolveRefusal> {
    Program::from_artifact(a).map_err(|why| EvolveRefusal::Uncompilable {
        version: a.version.clone(),
        why,
    })
}

/// [`render`], given the two stored artifacts.
pub async fn render_stored(
    pool: &Pool,
    a: &Artifact,
    b: &Artifact,
    options: &Choices,
    at: u64,
) -> Result<Rendered, EvolveRefusal> {
    let x = compiled(a)?;
    let y = compiled(b)?;
    // The `prev` marks against X (`plan-syntax-additions.md` §5): a mark the
    // step cannot make refuses the render; a landed one is a warning.
    let prev = ordo_schema::prev::check(Some(&x.schema), &y.schema);
    if !prev.errors.is_empty() {
        return Err(EvolveRefusal::Errors {
            from: a.version.clone(),
            to: b.version.clone(),
            errors: prev
                .errors
                .iter()
                .map(|i| PlanError {
                    code: "B4".into(),
                    change: format!("`{}`", i.mark.text()),
                    class: i.class.to_string(),
                    why: i.message.clone(),
                    fix: i.fix.clone(),
                })
                .collect(),
        });
    }
    let warnings: Vec<String> = prev
        .stale
        .iter()
        .map(|i| format!("{}: {}; {}", i.class, i.message, i.fix))
        .collect();
    let live = x_shape(pool, &a.ddl).await?;
    let changes = ordo_schema::diff_with(&live, Some(&x.schema), Some(&x.schema), &y.schema);
    // A fill for a field the step neither adds required nor makes required
    // is refused, as a stray `--places` is.
    for (entity, field) in options.fills.keys() {
        let named = changes.iter().any(|c| {
            matches!(c, Change::AddRequiredField { entity: e, field: f }
                | Change::RequireField { entity: e, field: f } if e == entity && f == field)
        });
        if !named {
            return Err(EvolveRefusal::Options(format!(
                "`{entity}.{field}` is not a field the step from `{}` to `{}` makes required; \
                 drop it from `--fill`",
                a.version, b.version
            )));
        }
    }

    // Every change, assessed by the rulings alone: probes are the run's,
    // per database.
    let assessed: Vec<sa::Assessed> = changes
        .iter()
        .filter(|c| !orphaned(c))
        .map(|c| sa::Assessed {
            change: c.clone(),
            refused: c.refusal().map(|r| vec![r.to_string()]).unwrap_or_default(),
            readers: Vec::new(),
            probe: None,
            extent: None,
        })
        .collect();
    let triaged = sa::triage_counted(
        &y.schema,
        &live,
        &assessed,
        options,
        &b.version,
        &BTreeMap::new(),
    )
    .map_err(|why| EvolveRefusal::Run {
        database: "render".into(),
        from: a.version.clone(),
        to: b.version.clone(),
        class: "render_failed",
        why,
        transient: false,
    })?;
    if !triaged.refused.is_empty() {
        return Err(EvolveRefusal::Errors {
            from: a.version.clone(),
            to: b.version.clone(),
            errors: triaged
                .refused
                .iter()
                .map(|r| PlanError {
                    code: r.change.code().to_string(),
                    change: r.change.to_string(),
                    class: r.class.to_string(),
                    why: r.why.clone(),
                    fix: fix_for(&r.change, r.class),
                })
                .collect(),
        });
    }
    // An option for a change the step does not have is refused: it would be
    // stored and mean nothing.
    for (entity, field) in options.places.keys() {
        let named = changes.iter().any(|c| {
            matches!(c, Change::ChangePlaces { entity: e, field: f, .. } if e == entity && f == field)
        });
        if !named {
            return Err(EvolveRefusal::Options(format!(
                "`{entity}.{field}` has no change of places in the step from `{}` to `{}`; \
                 drop it from `--places`",
                a.version, b.version
            )));
        }
    }

    let mut errors = mapping_errors(pool, &a.ddl, &triaged.backfills).await?;
    let mut functions = Vec::new();
    for bf in &triaged.backfills {
        match function_of(bf, &y, b) {
            Ok(Some(f)) => functions.push(f),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(EvolveRefusal::Errors {
            from: a.version.clone(),
            to: b.version.clone(),
            errors,
        });
    }

    let forward: Vec<String> = triaged
        .steps
        .iter()
        .map(|s| s.sql.trim_end().to_string())
        .filter(|s| !is_system_statement(s))
        .collect();
    let mut receipt = json!({
        "format": RECEIPT_FORMAT,
        "from": { "version": a.version, "sourceId": x.source_id },
        "to": { "version": b.version, "sourceId": y.source_id },
        "options": options_text(options),
        "changes": changes.iter().map(|c| change_json(c, &x, &y, options)).collect::<Vec<_>>(),
        "forward": forward,
    });
    // Present only when the step backfills, so every plan stored before
    // backfills existed keeps its digest.
    if !triaged.backfills.is_empty() {
        receipt["backfills"] = Value::Array(
            triaged
                .backfills
                .iter()
                .map(|bf| {
                    let mut j = bf.to_json();
                    let f = functions
                        .iter()
                        .find(|f| f.field == bf.field && f.entity == bf.entity);
                    if let Some(f) = f {
                        j["function"] = json!({
                            "name": f.name, "codeHash": f.code_hash,
                            "secrets": f.secrets, "hosts": f.hosts,
                        });
                    }
                    // Q14: a function that reads a secret or calls out may
                    // give staging and production different values.
                    j["reproducible"] =
                        json!(f.is_none_or(|f| f.secrets.is_empty() && f.hosts.is_empty()));
                    j["oldValues"] = json!(match bf.audit {
                        ordo_commandlog::BackfillAudit::Summarised =>
                            "summarised: the rows' old values are not in the command log; \
                             they come back only from a backup",
                        ordo_commandlog::BackfillAudit::Images =>
                            "images: every row's before and after image is in the command log",
                    });
                    j
                })
                .collect(),
        );
        receipt["after"] = Value::Array(
            triaged
                .after
                .iter()
                .map(|s| Value::String(s.sql.trim_end().to_string()))
                .collect(),
        );
    }
    let digest = digest_of(&receipt);
    let readers = reader_notices(&x, &y, &changes, at);
    Ok(Rendered {
        from: a.version.clone(),
        to: b.version.clone(),
        options: options_text(options),
        receipt,
        digest,
        readers,
        warnings,
    })
}

/// The live shape of an empty database built from `ddl`, in a shadow
/// namespace drawn for this render and dropped after.
async fn x_shape(pool: &Pool, ddl: &str) -> Result<ordo_schema::LiveShape, EvolveRefusal> {
    Ok(in_shadow(pool, ddl, |ns| async move {
        ordo_exec::migrate::live_shape(pool, &ns).await
    })
    .await??)
}

/// Run `f` against an empty database built from `ddl`, in a shadow
/// namespace drawn for it and dropped after.
async fn in_shadow<T, F, Fut>(
    pool: &Pool,
    ddl: &str,
    f: F,
) -> Result<Result<T, PoolError>, EvolveRefusal>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<T, PoolError>>,
{
    let mut nonce = [0u8; 8];
    getrandom::getrandom(&mut nonce).map_err(|e| EvolveRefusal::Store(format!("random: {e}")))?;
    let ns = format!(
        "{}_{}",
        ordo_exec::shape::DIFF_SHADOW,
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let out = match pool.build_shadow_in(&ns, ddl).await {
        Ok(()) => f(ns.clone()).await,
        Err(e) => Err(e),
    };
    let _ = pool
        .rows(&format!("DROP SCHEMA IF EXISTS {ns} CASCADE"))
        .await;
    Ok(out)
}

/// Each expression mapping, typed by the database against an empty
/// database of X: a mapping whose value the column's type cannot take, or
/// that reads a field X lacks, is a plan error, not a chunk that fails five
/// times on every tenant.
async fn mapping_errors(
    pool: &Pool,
    ddl: &str,
    backfills: &[sa::BackfillStep],
) -> Result<Vec<PlanError>, EvolveRefusal> {
    let typed: Vec<&sa::BackfillStep> = backfills.iter().filter(|b| b.sql.is_some()).collect();
    if typed.is_empty() {
        return Ok(Vec::new());
    }
    let owned: Vec<(String, String, String, String)> = typed
        .iter()
        .map(|b| {
            (
                b.entity.clone(),
                b.field.clone(),
                b.sql.clone().unwrap_or_default(),
                b.column_ty.clone(),
            )
        })
        .collect();
    let found = in_shadow(pool, ddl, |ns| async move {
        let mut errs = Vec::new();
        for (entity, field, sql, ty) in &owned {
            // The column as the expand adds it, then the backfill's own
            // assignment, planned and never run: the database applies the
            // assignment cast an `UPDATE` does.
            let t = format!("{ns}.{}", ordo_schema::ddl::q(entity));
            let c = ordo_schema::ddl::q(&format!("$bf_{field}"));
            let probe = format!(
                "ALTER TABLE {t} ADD COLUMN IF NOT EXISTS {c} {ty}; \
                 EXPLAIN UPDATE {t} SET {c} = ({sql})"
            );
            if let Err(e) = pool.rows(&probe).await {
                errs.push((entity.clone(), field.clone(), ty.clone(), e.to_string()));
            }
        }
        Ok(errs)
    })
    .await??;
    Ok(found
        .into_iter()
        .map(|(entity, field, ty, why)| PlanError {
            code: "B2".into(),
            change: format!("the fill of `{entity}.{field}`"),
            class: "mapping_invalid".into(),
            why: format!("the database refuses the mapping as `{ty}` over the version before the step: {why}"),
            fix: format!(
                "state a mapping over fields the version before declares, of the field's \
                 type: `--fill {entity}.{field}=<expression>`"
            ),
        })
        .collect())
}

/// A backfill function as the plan names it (D16, Q14).
#[derive(Debug, Clone)]
struct FnFill {
    entity: String,
    field: String,
    name: String,
    code_hash: String,
    secrets: Vec<String>,
    hosts: Vec<String>,
}

/// The function a fill names, checked against the version the step goes
/// to: it is declared there, its code is stored, and what the fill
/// declares it may use is within what the function itself declares.
fn function_of(
    bf: &sa::BackfillStep,
    y: &Program,
    b: &Artifact,
) -> Result<Option<FnFill>, PlanError> {
    let sa::FillMapping::Function {
        name,
        secrets,
        hosts,
    } = &bf.mapping
    else {
        return Ok(None);
    };
    let err = |why: String, fix: String| PlanError {
        code: bf.code.into(),
        change: format!("the fill of `{}.{}`", bf.entity, bf.field),
        class: "backfill_function_invalid".into(),
        why,
        fix,
    };
    let Some(decl) = y.schema.functions.iter().find(|f| f.name.name == *name) else {
        return Err(err(
            format!("version `{}` declares no function `{name}`", b.version),
            format!("declare `function {name}` in the version the step goes to, or fill with an expression"),
        ));
    };
    let doc: Value = serde_json::from_str(&b.artifact).unwrap_or(Value::Null);
    let code_hash = doc["functions"]
        .as_array()
        .and_then(|fs| fs.iter().find(|f| f["name"] == name.as_str()))
        .and_then(|f| f["code_hash"].as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            err(
                format!(
                    "function `{name}` has no stored code in version `{}`",
                    b.version
                ),
                "apply the version with `ordo apply`, which stores each function's code".into(),
            )
        })?;
    let own_secrets: Vec<&str> = decl.secrets.iter().map(|s| s.name.as_str()).collect();
    if let Some(s) = secrets.iter().find(|s| !own_secrets.contains(&s.as_str())) {
        return Err(err(
            format!("the fill declares secret `{s}`, which function `{name}` does not declare"),
            format!("add `secrets {s}` to `function {name}`, or drop it from the fill"),
        ));
    }
    let own_hosts: Vec<&str> = decl.egress.iter().map(|h| h.name.as_str()).collect();
    if let Some(h) = hosts
        .iter()
        .find(|h| !own_hosts.contains(&h.to_ascii_lowercase().as_str()))
    {
        return Err(err(
            format!("the fill declares host `{h}`, which function `{name}` does not declare"),
            format!("add `egress {h}` to `function {name}`, or drop it from the fill"),
        ));
    }
    Ok(Some(FnFill {
        entity: bf.entity.clone(),
        field: bf.field.clone(),
        name: name.clone(),
        code_hash,
        secrets: secrets.clone(),
        hosts: hosts.iter().map(|h| h.to_ascii_lowercase()).collect(),
    }))
}

/// What a developer does about a change a step cannot make.
fn fix_for(c: &Change, class: &str) -> String {
    match (c, class) {
        (Change::ChangePlaces { entity, field, .. }, _) => format!(
            "state it: `--places {entity}.{field}=rescale` or `--places {entity}.{field}=keep`"
        ),
        (
            Change::AddRequiredField { entity, field } | Change::RequireField { entity, field },
            "not_class_a" | "mapping_invalid",
        ) => format!(
            "state how every existing row gets a value: `--fill {entity}.{field}=<expression>` \
             over the row's own fields, or `--fill {entity}.{field}=fn <function>`"
        ),
        (_, "plan_refused") => "the change is refused by ruling; declare it another way".into(),
        _ => match c.class() {
            Class::B => "the menu does not offer this change yet (increments 10b, 11a); \
                         split it into an additive version first, or wait for the backfill"
                .into(),
            Class::C => "remove the constraint in its own version after the one that \
                         stops declaring it is active"
                .into(),
            _ => "render the step in two versions".into(),
        },
    }
}

/// One change as the receipt lists it.
fn change_json(c: &Change, x: &Program, y: &Program, options: &Choices) -> Value {
    let option = match c {
        Change::ChangePlaces { entity, field, .. } => options
            .places_for(entity, field)
            .map(|p| Value::String(p.as_str().to_string()))
            .unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let (reverse, why) = reverse_of(c, x, y);
    json!({
        "code": c.code(),
        "class": c.class().to_string(),
        "change": c.to_string(),
        "option": option,
        "orphaned": orphaned(c),
        "reverse": reverse,
        "reverseNote": why,
    })
}

/// A change's reverse: the statements, or none and why.
fn reverse_of(c: &Change, x: &Program, y: &Program) -> (Vec<String>, Option<String>) {
    use ordo_schema::ddl::q;
    let drop_constraint = |entity: &str, object: &str| {
        format!("ALTER TABLE {} DROP CONSTRAINT {};", q(entity), q(object))
    };
    match c {
        Change::AddEntity { entity } => (
            vec![format!("DROP TABLE {};", q(entity))],
            Some("drops the table and every row written to it".into()),
        ),
        Change::AddOptionalField { entity, field } | Change::AddConstantField { entity, field, .. } => {
            let mut out = Vec::new();
            if let Some(f) = y.schema.native(entity).and_then(|e| e.field(field)) {
                out.push(format!(
                    "ALTER TABLE {} DROP COLUMN {};",
                    q(entity),
                    q(&f.column())
                ));
                if f.indexed {
                    out.push(format!(
                        "ALTER TABLE {} DROP COLUMN {};",
                        q(entity),
                        q(&f.index_column())
                    ));
                }
            }
            (out, Some("drops the column and what was written to it".into()))
        }
        Change::AddIndex { object, .. } => {
            (vec![format!("DROP INDEX CONCURRENTLY {};", q(object))], None)
        }
        Change::AddUnique { entity, object, .. }
        | Change::AddCompositeUnique { entity, object, .. }
        | Change::AddEnum { entity, object, .. }
        | Change::AddForeignKey { entity, object, .. }
        | Change::AddCheck { entity, object, .. } => (vec![drop_constraint(entity, object)], None),
        Change::DropField { .. } | Change::DropEntity { .. } => (
            Vec::new(),
            Some("orphaned: the column is kept, so the reverse serves it again".into()),
        ),
        // The rename read backwards: what the new version wrote is copied
        // back into the old column, which was kept, and the new one goes.
        Change::RenameField {
            entity,
            from,
            field,
        } => {
            let t = q(entity);
            let mut out = vec![format!("UPDATE {t} SET {} = {};", q(from), q(field))];
            if x
                .schema
                .native(entity)
                .and_then(|e| e.field(from))
                .is_some_and(|f| !f.optional)
            {
                out.push(format!(
                    "ALTER TABLE {t} ALTER COLUMN {} SET NOT NULL;",
                    q(from)
                ));
            }
            out.push(format!("ALTER TABLE {t} DROP COLUMN {};", q(field)));
            (
                out,
                Some("the rename read backwards: the old column was kept, and gets back what was written to the new one".into()),
            )
        }
        Change::ChangePlaces { .. } | Change::ChangeType { .. } => (
            Vec::new(),
            Some("the reverse is the stored plan from the later version back, with the same option".into()),
        ),
        _ => (
            Vec::new(),
            Some("no reverse is generated: accept this one way change; revert requires this or manual intervention".into()),
        ),
    }
}

/// F1: each reader of a table the step retypes or removes a field of, or
/// removes whole. The read app cannot see the reader's file, so every
/// reader of the table is listed, whether or not it maps the field.
fn reader_notices(x: &Program, y: &Program, changes: &[Change], at: u64) -> Vec<ReaderNotice> {
    let ends = at + SOAK_DEFAULT_DAYS * 86_400;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for c in changes {
        let (table, field) = match c {
            Change::DropField { entity, field }
            | Change::RenameField {
                entity,
                from: field,
                ..
            }
            | Change::ChangeType { entity, field, .. }
            | Change::SealField { entity, field }
            | Change::UnsealField { entity, field } => (entity, Some(field.clone())),
            Change::DropEntity { entity } => (entity, None),
            Change::RenameEntity { from, .. } => (from, None),
            _ => continue,
        };
        for r in x.schema.readers.iter().chain(y.schema.readers.iter()) {
            if !r.tables.iter().any(|t| t == table) {
                continue;
            }
            if !seen.insert((r.name.clone(), table.clone(), field.clone())) {
                continue;
            }
            out.push(ReaderNotice {
                reader: r.name.clone(),
                table: table.clone(),
                field: field.clone(),
                change: c.to_string(),
                grant_ends_not_before: ends,
            });
        }
    }
    out
}

// ── the store ───────────────────────────────────────────────────────────

/// One stored plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPlan {
    pub seq: i64,
    pub from: String,
    pub to: String,
    pub digest: String,
    pub options: String,
    /// The receipt, canonical JSON.
    pub receipt: String,
    pub actor: String,
    pub at: u64,
}

impl StoredPlan {
    pub fn to_json(&self) -> Value {
        json!({
            "seq": self.seq, "from": self.from, "to": self.to, "digest": self.digest,
            "options": self.options, "actor": self.actor, "at": self.at,
            "receipt": serde_json::from_str::<Value>(&self.receipt).unwrap_or(Value::Null),
        })
    }

    pub fn from_json(v: &Value) -> Option<StoredPlan> {
        Some(StoredPlan {
            seq: v["seq"].as_i64()?,
            from: v["from"].as_str()?.to_string(),
            to: v["to"].as_str()?.to_string(),
            digest: v["digest"].as_str()?.to_string(),
            options: v["options"].as_str().unwrap_or_default().to_string(),
            receipt: if v["receipt"].is_null() {
                String::new()
            } else {
                ordo_plan::artifact::to_bytes(&v["receipt"])
            },
            actor: v["actor"].as_str().unwrap_or_default().to_string(),
            at: v["at"].as_u64().unwrap_or(0),
        })
    }
}

/// `$plans`' client. The one reader and writer of the table.
pub struct Plans<'a> {
    pool: &'a Pool,
}

const PLAN_COLUMNS: &str =
    "\"seq\", \"from_version\", \"to_version\", \"digest\", \"options\", \"receipt\", \"actor\", \"at\"";

fn plan_row(r: &[Option<String>]) -> Option<StoredPlan> {
    let g = |i: usize| r.get(i).cloned().flatten();
    Some(StoredPlan {
        seq: g(0)?.parse().ok()?,
        from: g(1)?,
        to: g(2)?,
        digest: g(3)?,
        options: g(4).unwrap_or_default(),
        receipt: g(5).unwrap_or_default(),
        actor: g(6).unwrap_or_default(),
        at: g(7).and_then(|s| s.parse().ok()).unwrap_or(0),
    })
}

impl<'a> Plans<'a> {
    pub fn on(pool: &'a Pool) -> Plans<'a> {
        Plans { pool }
    }

    pub async fn ensure(&self) -> Result<(), EvolveRefusal> {
        Versions::on(self.pool).ensure().await?;
        self.pool.rows(PLANS_DDL).await?;
        Ok(())
    }

    /// Store a rendered plan. The same plan stored again answers the row it
    /// already has, and `false`.
    pub async fn store(
        &self,
        r: &Rendered,
        actor: &str,
        at: u64,
    ) -> Result<(StoredPlan, bool), EvolveRefusal> {
        self.ensure().await?;
        if let Some(p) = self.with_digest(&r.from, &r.to, &r.digest).await? {
            return Ok((p, false));
        }
        let rows = self
            .pool
            .rows(&format!(
                "INSERT INTO \"$plans\" (\"from_version\", \"to_version\", \"digest\", \
                 \"options\", \"receipt\", \"actor\", \"at\") VALUES ({}, {}, {}, {}, {}, {}, {at}) \
                 ON CONFLICT (\"from_version\", \"to_version\", \"digest\") DO NOTHING \
                 RETURNING {PLAN_COLUMNS}",
                sql_lit(&r.from),
                sql_lit(&r.to),
                sql_lit(&r.digest),
                sql_lit(&r.options),
                sql_lit(&ordo_plan::artifact::to_bytes(&r.receipt)),
                sql_lit(actor),
            ))
            .await?;
        match rows.first().and_then(|r| plan_row(r)) {
            Some(p) => Ok((p, true)),
            // Stored by a racing call a moment ago.
            None => self
                .with_digest(&r.from, &r.to, &r.digest)
                .await?
                .map(|p| (p, false))
                .ok_or_else(|| {
                    EvolveRefusal::Store("the plan was neither stored nor found".into())
                }),
        }
    }

    async fn with_digest(
        &self,
        from: &str,
        to: &str,
        digest: &str,
    ) -> Result<Option<StoredPlan>, EvolveRefusal> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT {PLAN_COLUMNS} FROM \"$plans\" WHERE \"from_version\" = {} \
                 AND \"to_version\" = {} AND \"digest\" = {}",
                sql_lit(from),
                sql_lit(to),
                sql_lit(digest)
            ))
            .await?;
        Ok(rows.first().and_then(|r| plan_row(r)))
    }

    /// Every stored plan, oldest first. A database with no `$plans` has
    /// none.
    pub async fn all(&self) -> Result<Vec<StoredPlan>, EvolveRefusal> {
        match self
            .pool
            .rows(&format!(
                "SELECT {PLAN_COLUMNS} FROM \"$plans\" ORDER BY \"seq\""
            ))
            .await
        {
            Ok(rows) => Ok(rows.iter().filter_map(|r| plan_row(r)).collect()),
            Err(e) if e.is_missing_table() => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// The plan an edge runs: the newest stored for it.
    pub async fn latest(&self, from: &str, to: &str) -> Result<Option<StoredPlan>, EvolveRefusal> {
        Ok(self
            .all()
            .await?
            .into_iter()
            .rfind(|p| p.from == from && p.to == to))
    }
}

// ── evolve: render, check, store ────────────────────────────────────────

/// What [`evolve`] did.
#[derive(Debug, Clone)]
pub struct Evolved {
    pub rendered: Rendered,
    pub plan: StoredPlan,
    /// `false` when the same plan was stored already.
    pub stored: bool,
    /// The sign-in methods the step removes, when it removes any: approved,
    /// or leaving nobody without a method.
    pub sign_in: Option<crate::precheck::SignInRemoval>,
}

impl Evolved {
    /// The op's answer.
    pub fn to_json(&self) -> Value {
        json!({
            "from": self.rendered.from,
            "to": self.rendered.to,
            "digest": self.rendered.digest,
            "stored": self.stored,
            "seq": self.plan.seq,
            "options": self.rendered.options,
            "receipt": self.rendered.receipt,
            "readers": self.rendered.readers.iter().map(|r| json!({
                "reader": r.reader, "table": r.table, "field": r.field,
                "change": r.change, "grantEndsNotBefore": r.grant_ends_not_before,
                "line": r.line(),
            })).collect::<Vec<_>>(),
            "warnings": self.rendered.warnings,
            "signIn": self.sign_in.as_ref().map(crate::precheck::SignInRemoval::to_json),
        })
    }
}

/// **`ordo evolve --from X --to Y`**: render the step, refuse it if it has
/// errors or if `expect` names another digest, and store it. A step that
/// removes a sign-in method some people rely on is refused: see
/// [`evolve_approved`].
pub async fn evolve(
    pool: &Pool,
    from: &str,
    to: &str,
    options: &Choices,
    expect: Option<&str>,
    actor: &str,
    at: u64,
) -> Result<Evolved, EvolveRefusal> {
    evolve_approved(pool, from, to, options, expect, None, actor, at).await
}

/// [`evolve`], with the deployment owner's approval of a sign-in method
/// removal (ruled 2026-11-23, Q6). `approved` names how many people the
/// owner accepts leaving with no method. It must be the count the check
/// finds now, so an approval never covers more people than it named.
#[allow(clippy::too_many_arguments)]
pub async fn evolve_approved(
    pool: &Pool,
    from: &str,
    to: &str,
    options: &Choices,
    expect: Option<&str>,
    approved: Option<u64>,
    actor: &str,
    at: u64,
) -> Result<Evolved, EvolveRefusal> {
    let rendered = render(pool, from, to, options, at).await?;
    if let Some(expected) = expect.map(str::trim).filter(|e| !e.is_empty()) {
        if expected != rendered.digest {
            return Err(EvolveRefusal::DigestMismatch {
                from: rendered.from,
                to: rendered.to,
                expected: expected.to_string(),
                rendered: rendered.digest,
            });
        }
    }
    let v = Versions::on(pool);
    let (Some(a), Some(b)) = (v.get(&rendered.from).await?, v.get(&rendered.to).await?) else {
        return Err(VersionRefusal::UnknownVersion(rendered.from.clone()).into());
    };
    let sign_in = crate::precheck::sign_in_removal(
        pool,
        &compiled(&a)?,
        &compiled(&b)?,
        &rendered.from,
        &rendered.to,
    )
    .await?;
    if let Some(removal) = &sign_in {
        if removal.people > 0 && approved != Some(removal.people) {
            return Err(EvolveRefusal::SignInRemoval {
                removal: removal.clone(),
                approved,
            });
        }
    }
    let (plan, stored) = Plans::on(pool).store(&rendered, actor, at).await?;
    Ok(Evolved {
        rendered,
        plan,
        stored,
        sign_in,
    })
}

// ── the path ────────────────────────────────────────────────────────────

/// **The stored path from `from` to `to`**: the fewest stored steps, each
/// edge's newest plan. Never a diff of the two ends. Empty when they are
/// the same version.
pub async fn path(pool: &Pool, from: &str, to: &str) -> Result<Vec<StoredPlan>, EvolveRefusal> {
    if from == to {
        return Ok(Vec::new());
    }
    let mut edges: BTreeMap<String, BTreeMap<String, StoredPlan>> = BTreeMap::new();
    for p in Plans::on(pool).all().await? {
        // Newest wins: `all` is oldest first.
        edges
            .entry(p.from.clone())
            .or_default()
            .insert(p.to.clone(), p);
    }
    let mut came: BTreeMap<String, StoredPlan> = BTreeMap::new();
    let mut queue = VecDeque::from([from.to_string()]);
    let mut seen = BTreeSet::from([from.to_string()]);
    while let Some(at) = queue.pop_front() {
        if at == to {
            break;
        }
        // Each version's next steps in stored order, so the path chosen
        // between two of equal length is the same every time.
        let mut next: Vec<&StoredPlan> = edges
            .get(&at)
            .map(|m| m.values().collect())
            .unwrap_or_default();
        next.sort_by_key(|p| p.seq);
        for p in next {
            if seen.insert(p.to.clone()) {
                came.insert(p.to.clone(), p.clone());
                queue.push_back(p.to.clone());
            }
        }
    }
    let mut steps = Vec::new();
    let mut at = to.to_string();
    while at != from {
        let Some(p) = came.get(&at) else {
            return Err(EvolveRefusal::NoPath {
                from: from.to_string(),
                to: to.to_string(),
            });
        };
        steps.push(p.clone());
        at = p.from.clone();
    }
    steps.reverse();
    Ok(steps)
}

// ── the run ─────────────────────────────────────────────────────────────

/// What one step did on one database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepRun {
    /// The database already recorded the step's to-version.
    Current,
    /// The step ran: how many statements were sent, and each backfill's
    /// report (10b).
    Ran {
        statements: usize,
        backfills: Vec<Value>,
    },
    /// A backfill stopped at its budget (`backfill.budget`). The database
    /// still records the version before the step; running the step again
    /// resumes from the cursor.
    Backfilling {
        statements: usize,
        backfills: Vec<Value>,
    },
}

impl StepRun {
    pub fn statements(&self) -> usize {
        match self {
            StepRun::Current => 0,
            StepRun::Ran { statements, .. } | StepRun::Backfilling { statements, .. } => {
                *statements
            }
        }
    }

    pub fn backfills(&self) -> &[Value] {
        match self {
            StepRun::Current => &[],
            StepRun::Ran { backfills, .. } | StepRun::Backfilling { backfills, .. } => backfills,
        }
    }
}

/// What a step runs its backfills with (10b).
#[derive(Clone, Default)]
pub struct StepContext {
    pub settings: crate::backfill::BackfillSettings,
    /// Runs backfill functions (B6). `None` refuses a function fill.
    pub functions: Option<std::sync::Arc<dyn crate::backfill::BackfillFunctions>>,
}

/// The database's recorded version.
async fn recorded(target: &Pool) -> Result<Option<String>, EvolveRefusal> {
    match ordo_exec::spacedb::recorded_version(target).await {
        Ok(ordo_exec::spacedb::TargetVersion::At(v)) => Ok(Some(v)),
        Ok(ordo_exec::spacedb::TargetVersion::Unknown) => Ok(None),
        Err(e) => Err(EvolveRefusal::Run {
            database: String::new(),
            from: String::new(),
            to: String::new(),
            class: "unreachable",
            why: e.to_string(),
            transient: true,
        }),
    }
}

/// **Run one stored step on one database.** `registry` is the deployment's
/// own database, where the stored versions and plans are; `target` is the
/// database the step runs on, which may be the same. `label` names the
/// target in a refusal. The caller holds the apply lock.
///
/// In order: the stale check (the target records the step's from-version),
/// the plan digest re-rendered here against the stored one, then the step
/// rendered again on the target with the stored options and run. The
/// target's live shape must hold all of the from-version (the drift check
/// in the apply). Ordo's own tables are written as the run renders them.
pub async fn run_step(
    registry: &Pool,
    target: &Pool,
    label: &str,
    plan: &StoredPlan,
    actor: &str,
) -> Result<StepRun, EvolveRefusal> {
    run_step_with(
        registry,
        target,
        label,
        plan,
        actor,
        &StepContext::default(),
    )
    .await
}

/// [`run_step`], with the backfills' settings and function runner.
///
/// A step that fills a field (10b) runs in three parts: the expand with the
/// rest of the step, each backfill to its end and verified, then the
/// `NOT NULL` each filled field declares. A kill at any point leaves the
/// database recording the version before the step, and running the step
/// again resumes each backfill from its cursor.
pub async fn run_step_with(
    registry: &Pool,
    target: &Pool,
    label: &str,
    plan: &StoredPlan,
    actor: &str,
    ctx: &StepContext,
) -> Result<StepRun, EvolveRefusal> {
    let with_label = |mut e: EvolveRefusal| {
        if let EvolveRefusal::Run {
            database, from, to, ..
        } = &mut e
        {
            *database = label.to_string();
            *from = plan.from.clone();
            *to = plan.to.clone();
        }
        e
    };
    let now = recorded(target).await.map_err(with_label)?;
    if now.as_deref() == Some(plan.to.as_str()) {
        return Ok(StepRun::Current);
    }
    if now.as_deref() != Some(plan.from.as_str()) {
        return Err(EvolveRefusal::Stale {
            database: label.to_string(),
            recorded: now,
            expected: plan.from.clone(),
        });
    }
    let v = Versions::on(registry);
    let (Some(a), Some(b)) = (v.get(&plan.from).await?, v.get(&plan.to).await?) else {
        return Err(VersionRefusal::UnknownVersion(plan.from.clone()).into());
    };
    let options = parse_options(&plan.options)?;
    // The stored digest against this release's rendering (D3).
    let again = render_stored(registry, &a, &b, &options, crate::runtime::now_secs()).await?;
    if again.digest != plan.digest {
        return Err(EvolveRefusal::DigestMismatch {
            from: plan.from.clone(),
            to: plan.to.clone(),
            expected: plan.digest.clone(),
            rendered: again.digest,
        });
    }
    let x = compiled(&a)?;
    let y = compiled(&b)?;
    let run_err = |class: &'static str, why: String, transient: bool| EvolveRefusal::Run {
        database: label.to_string(),
        from: plan.from.clone(),
        to: plan.to.clone(),
        class,
        why,
        transient,
    };
    let mut p = sa::plan_with(target, &y, &options, Some(&x.schema))
        .await
        .map_err(|e| run_err("internal", e, true))?;
    // The step starts from X on every database: X is the baseline its
    // drift is measured against and the version its record says it left.
    p.prior = Some(x.schema.clone());
    p.prior_doc = serde_json::from_str(&a.artifact).ok();
    p.deployed = Some(a.clone());
    // A removed field or entity is orphaned, never dropped.
    p.assessed.retain(|a| !orphaned(&a.change));
    let mut refusals = p.external.refusals();
    for a in &mut p.assessed {
        a.readers.clear();
        a.refused.retain(|r| r != "drop_read_by_active_op");
        for r in &a.refused {
            let class = r.split(" — ").next().unwrap_or(r);
            refusals.push(format!("{class} on `{}`", a.change.entity()));
        }
    }
    p.refusals = refusals;
    // The pre-checks on this database (27w): every issue, each with its
    // fix, rather than the first refusal the apply meets.
    let issues = crate::precheck::issues_of(label, &p, &plan.to);
    if !issues.is_empty() {
        return Err(EvolveRefusal::PreChecks {
            from: plan.from.clone(),
            to: plan.to.clone(),
            issues,
        });
    }
    let (statements, backfills, after) = match sa::apply_planned(target, &p, actor, OP).await {
        Ok(sa::Applied::Additive {
            steps,
            backfills,
            after,
            ..
        }) => (steps.len(), backfills, after),
        Ok(sa::Applied::Nothing) => (0, Vec::new(), Vec::new()),
        Ok(sa::Applied::Created(c)) => (c.tables, Vec::new(), Vec::new()),
        Err(sa::ApplyRefusal::ServingDrifted { serving, drift }) => {
            return Err(EvolveRefusal::Drifted {
                database: label.to_string(),
                version: serving,
                drift: drift.iter().map(|c| format!("{} {c}", c.code())).collect(),
            })
        }
        Err(sa::ApplyRefusal::Ghost { serving }) => {
            return Err(EvolveRefusal::Drifted {
                database: label.to_string(),
                version: serving,
                drift: vec!["none of its tables exist".into()],
            })
        }
        Err(sa::ApplyRefusal::Plan) => {
            return Err(run_err("plan_refused", p.refusals.join("; "), false))
        }
        Err(sa::ApplyRefusal::NotApplied(t)) => {
            return Err(run_err(
                "not_applied",
                t.refused
                    .iter()
                    .map(|r| format!("{} ({}): {}", r.class, r.change, r.why))
                    .collect::<Vec<_>>()
                    .join("; "),
                false,
            ))
        }
        Err(sa::ApplyRefusal::ExecFailed(f)) => {
            return Err(run_err("execute_failed", f.failure.to_string(), false))
        }
        Err(other) => return Err(run_err(other.class(), format!("{other:?}"), true)),
    };
    // The backfills (10b), each to its end and verified, then what lands
    // after them. A budget that stops one leaves the step for a run again.
    let mut reports = Vec::new();
    for bf in &backfills {
        // Whether the column is new is the step's fact, not this run's: a
        // run again after the expand sees the column there (B5), and the
        // step still added it (B2), so verify still checks every row holds
        // the mapping.
        let mut bf = bf.clone();
        let rendered_new = again.receipt["backfills"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| {
                r["entity"] == bf.entity.as_str()
                    && r["field"] == bf.field.as_str()
                    && r["code"] == "B2"
            });
        if rendered_new {
            bf.code = "B2";
        }
        let bf = &bf;
        let Some(entity) = y.schema.native(&bf.entity) else {
            return Err(run_err(
                "internal",
                format!("`{}` is not an entity of `{}`", bf.entity, plan.to),
                false,
            ));
        };
        let worker = crate::backfill::Backfill {
            pool: target,
            step: &plan.digest,
            version: &plan.to,
            fill: bf,
            entity,
            settings: &ctx.settings,
            functions: ctx.functions.clone(),
        };
        let bf_err =
            |e: crate::backfill::BackfillRefusal| run_err(e.class(), e.to_string(), e.transient());
        let progress = worker.run().await.map_err(bf_err)?;
        let mut report = json!({
            "entity": bf.entity, "field": bf.field, "mapping": bf.mapping.text(),
            "audit": bf.audit.as_str(), "progress": progress.to_json(),
        });
        if !progress.ended {
            // Stopped at its budget, with rows still after the cursor.
            reports.push(report);
            return Ok(StepRun::Backfilling {
                statements,
                backfills: reports,
            });
        }
        let verified = worker.verify_and_close().await.map_err(bf_err)?;
        report["verified"] = json!({
            "rows": verified.rows,
            "already": verified.already,
            "spaces": verified.spaces.iter().map(|s| json!({
                "space": s.space, "rows": s.rows, "content": s.content,
            })).collect::<Vec<_>>(),
        });
        reports.push(report);
    }
    if !after.is_empty() {
        sa::execute(target, &after)
            .await
            .map_err(|f| run_err("execute_failed", f.to_string(), false))?;
    }
    // What was applied is checked rather than assumed: the record below
    // says the database holds Y.
    match sa::schema_state(target, &y.schema).await {
        Ok(sa::SchemaState::Matches { .. }) => {}
        Ok(other) => {
            return Err(run_err(
                "still_behind",
                format!(
                    "the database still lacks part of `{}` after the step: {other:?}",
                    plan.to
                ),
                false,
            ))
        }
        Err(e) => return Err(run_err("internal", e, true)),
    }
    if recorded(target).await.map_err(with_label)?.as_deref() != Some(plan.to.as_str()) {
        sa::record(
            target,
            &sa::Record {
                kind: ordo_exec::migrate::kind::VERSION,
                from: Some(&plan.from),
                to: &plan.to,
                actor,
                op: OP,
                changes: &format!("stored plan {} ({})", plan.seq, plan.digest),
                ddl: "",
                phase: ordo_exec::migrate::phase::APPLIED,
                note: "",
            },
        )
        .await
        .map_err(|e| run_err("internal", e, true))?;
    }
    Ok(StepRun::Ran {
        statements,
        backfills: reports,
    })
}

/// What [`replay`] did on one database.
#[derive(Debug, Clone)]
pub struct Replayed {
    /// The version the database recorded before.
    pub from: String,
    pub to: String,
    /// Each step run, in order, with what it did.
    pub steps: Vec<(StoredPlan, StepRun)>,
}

impl Replayed {
    pub fn to_json(&self) -> Value {
        json!({
            "from": self.from,
            "to": self.to,
            "steps": self.steps.iter().map(|(p, r)| json!({
                "from": p.from, "to": p.to, "seq": p.seq, "digest": p.digest,
                "statements": r.statements() as i64,
                "ran": matches!(r, StepRun::Ran { .. }),
                "backfilling": matches!(r, StepRun::Backfilling { .. }),
                "backfills": r.backfills(),
            })).collect::<Vec<_>>(),
        })
    }

    /// A backfill stopped at its budget: the database is still on the
    /// step's from-version.
    pub fn backfilling(&self) -> bool {
        self.steps
            .last()
            .is_some_and(|(_, r)| matches!(r, StepRun::Backfilling { .. }))
    }
}

/// **Replay the stored path on one database**, from the version it records
/// to `to`, one stored step at a time. Stops at the first refusal; the
/// steps before it stay run, and the database records the last one.
pub async fn replay(
    registry: &Pool,
    target: &Pool,
    label: &str,
    to: &str,
    actor: &str,
) -> Result<Replayed, EvolveRefusal> {
    replay_with(registry, target, label, to, actor, &StepContext::default()).await
}

/// [`replay`], with the backfills' settings and function runner. A step
/// whose backfill stops at its budget ends the replay there.
pub async fn replay_with(
    registry: &Pool,
    target: &Pool,
    label: &str,
    to: &str,
    actor: &str,
    ctx: &StepContext,
) -> Result<Replayed, EvolveRefusal> {
    let to = Versions::on(registry).resolve(to).await?;
    let Some(from) = recorded(target).await? else {
        return Err(EvolveRefusal::Stale {
            database: label.to_string(),
            recorded: None,
            expected: "a stored version".into(),
        });
    };
    let steps = path(registry, &from, &to).await?;
    let mut out = Vec::with_capacity(steps.len());
    for p in steps {
        let r = run_step_with(registry, target, label, &p, actor, ctx).await?;
        let stop = matches!(r, StepRun::Backfilling { .. });
        out.push((p, r));
        if stop {
            break;
        }
    }
    Ok(Replayed {
        from,
        to,
        steps: out,
    })
}

/// The receipt's changes, each as one line, for a person.
pub fn receipt_lines(receipt: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for c in receipt["changes"].as_array().into_iter().flatten() {
        let mut line = format!(
            "{} [{}] {}",
            c["code"].as_str().unwrap_or("?"),
            c["class"].as_str().unwrap_or("?"),
            c["change"].as_str().unwrap_or("?")
        );
        if let Some(o) = c["option"].as_str() {
            line.push_str(&format!(" · option: {o}"));
        }
        if c["orphaned"].as_bool() == Some(true) {
            line.push_str(" · orphaned, kept");
        }
        out.push(line);
        for r in c["reverse"].as_array().into_iter().flatten() {
            out.push(format!("    reverse: {}", r.as_str().unwrap_or_default()));
        }
        if let Some(n) = c["reverseNote"].as_str() {
            out.push(format!("    reverse note: {n}"));
        }
    }
    for s in receipt["forward"].as_array().into_iter().flatten() {
        out.push(format!("forward: {}", s.as_str().unwrap_or_default()));
    }
    for b in receipt["backfills"].as_array().into_iter().flatten() {
        let mut line = format!(
            "backfill: {}.{} = {} · audit: {}",
            b["entity"].as_str().unwrap_or("?"),
            b["field"].as_str().unwrap_or("?"),
            b["mapping"].as_str().unwrap_or("?"),
            b["audit"].as_str().unwrap_or("?"),
        );
        if let Some(f) = b["function"].as_object() {
            let list = |k: &str| {
                f.get(k)
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "none".into())
            };
            line.push_str(&format!(
                " · function code {} · secrets: {} · hosts: {}",
                f.get("codeHash").and_then(|v| v.as_str()).unwrap_or("?"),
                list("secrets"),
                list("hosts")
            ));
        }
        if b["reproducible"].as_bool() == Some(false) {
            line.push_str(" · not reproducible: staging may get other values");
        }
        out.push(line);
        if let Some(n) = b["oldValues"].as_str() {
            out.push(format!("    old values: {n}"));
        }
    }
    for s in receipt["after"].as_array().into_iter().flatten() {
        out.push(format!(
            "after backfill: {}",
            s.as_str().unwrap_or_default()
        ));
    }
    out
}

// ── the door ────────────────────────────────────────────────────────────

/// `ordo evolve --from X --to Y`: render, check and store a plan; with
/// `run`, run it on the deployment's own database too.
pub const EVOLVE: &str = "std.deployment.evolve";
/// The stored plans; with `from` and `to`, that edge's.
pub const PLANS: &str = "std.deployment.plans";
/// `ordo evolve --tenant T`: replay the stored path on the tenant's
/// database, then pin the tenant.
pub const REPLAY: &str = "std.deployment.replay";
/// `ordo rollback --dry-run`: whether the version a rollback would return
/// to can still read its externals and peers here (F4).
pub const ROLLBACK_CHECK: &str = "std.deployment.rollbackCheck";

/// What [`rollback_check`] found.
#[derive(Debug, Clone)]
pub struct RollbackCheck {
    pub version: String,
    pub active: Option<String>,
    /// One line per external or peer table the version can no longer read.
    pub refusals: Vec<String>,
    pub notes: Vec<String>,
}

impl RollbackCheck {
    pub fn to_json(&self) -> Value {
        json!({
            "version": self.version, "active": self.active,
            "refusals": self.refusals, "notes": self.notes,
            "ok": self.refusals.is_empty(),
        })
    }
}

/// **F4: the check a rollback runs before it returns to `version`.** A
/// version pin does not cover a foreign table or a peer's columns, so the
/// version a rollback returns to is checked against them as they are now
/// (`plan-evolution-federation.md` §3.4, §4). Reads; changes nothing.
pub async fn rollback_check(pool: &Pool, version: &str) -> Result<RollbackCheck, EvolveRefusal> {
    let v = Versions::on(pool);
    let version = v.resolve(version).await?;
    let Some(a) = v.get(&version).await? else {
        return Err(VersionRefusal::UnknownVersion(version).into());
    };
    let program = compiled(&a)?;
    let report = crate::external_check::check(pool, &program.schema)
        .await
        .map_err(EvolveRefusal::Store)?;
    Ok(RollbackCheck {
        version,
        active: v.active_version().await?,
        refusals: report.refusals(),
        notes: report.notes,
    })
}

/// The three ops' specs.
pub fn specs() -> Vec<ordo_api::OpSpec> {
    use ordo_api::{
        FieldSpec, NotSyncable, OpCost, OpKind, OpSpec, ParamSpec, ParamTy, RefineTier, Syncability,
    };
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let flag = |name: &str| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Bool,
        required: false,
        bounds: None,
    };
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, fields: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: fields.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        reads_external: false,
        sync: Syncability::NotSyncable(if kind == OpKind::Read {
            NotSyncable::NoAnchor
        } else {
            NotSyncable::NotARead
        }),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            EVOLVE,
            OpKind::Write,
            vec![
                text("from", 128, true),
                text("to", 128, true),
                // `entity.field=rescale|keep`, comma-separated.
                text("places", 4096, false),
                // `entity.field=<expression>` or `entity.field=fn <name>
                // [secrets a,b] [hosts h,i]`, `;`-separated (10b).
                text("fill", 4096, false),
                // `entity.field`, comma-separated: fills audited with full
                // per-row images instead of a summary.
                text("auditImages", 1024, false),
                // The digest the step rendered to where it was rehearsed.
                text("expectDigest", 128, false),
                // Run the stored plan on the deployment's own database.
                flag("run"),
                // The deployment owner's approval of a sign-in method
                // removal: the number of people the warning named (Q6).
                text("approveSignInRemoval", 20, false),
            ],
            &[
                "from",
                "to",
                "digest",
                "stored",
                "seq",
                "options",
                "receipt",
                "readers",
                "signIn",
                "prechecks",
                "ran",
            ],
        ),
        spec(
            PLANS,
            OpKind::Read,
            vec![text("from", 128, false), text("to", 128, false)],
            &["plans"],
        ),
        spec(
            ROLLBACK_CHECK,
            OpKind::Read,
            vec![text("version", 128, true)],
            &["version", "active", "refusals", "notes", "ok"],
        ),
        spec(
            REPLAY,
            OpKind::Write,
            vec![
                text("tenant", 128, true),
                // Absent: the active version.
                text("to", 128, false),
            ],
            &["tenant", "from", "to", "steps", "pinned"],
        ),
    ]
}

/// Run one of the three. The caller is the deployment owner on the home
/// space (`deploy_ops::execute` checked).
pub async fn execute(
    ops: &crate::apply_op::ApplyOps,
    name: &str,
    bound: &ordo_api::BoundParams,
    caller: &str,
) -> Result<Value, ExecError> {
    let text = |k: &str| {
        bound
            .0
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let pool = &ops.pool;
    match name {
        EVOLVE => {
            const WHAT: &str = "the plan was not stored";
            let options = typed_options(
                text("places").unwrap_or_default(),
                text("fill").unwrap_or_default(),
                text("auditImages").unwrap_or_default(),
            )
            .map_err(|e| e.to_exec_error(WHAT))?;
            let run = bound
                .0
                .get("run")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let lock = if run {
                Some(take_lock(pool).await.map_err(|e| e.to_exec_error(WHAT))?)
            } else {
                None
            };
            let approved = match text("approveSignInRemoval") {
                None => None,
                Some(n) => Some(n.parse::<u64>().map_err(|_| {
                    EvolveRefusal::Options(format!(
                        "`--approve-sign-in-removal {n}` is not a count: name the number of \
                         people the warning gave"
                    ))
                    .to_exec_error(WHAT)
                })?),
            };
            let done = evolve_approved(
                pool,
                text("from").unwrap_or_default(),
                text("to").unwrap_or_default(),
                &options,
                text("expectDigest"),
                approved,
                caller,
                crate::runtime::now_secs(),
            )
            .await
            .map_err(|e| e.to_exec_error(WHAT))?;
            if let Some(removal) = done.sign_in.as_ref().filter(|r| r.people > 0) {
                // Q6: the deployment owner's approval, on the stream.
                ops.record(
                    caller,
                    EVOLVE,
                    "sign-in-removal-approved",
                    format!(
                        "from={} to={} removed={} people={} by={caller}",
                        done.plan.from,
                        done.plan.to,
                        removal.removed.join("+"),
                        removal.people
                    ),
                );
            }
            if done.stored {
                ops.record(
                    caller,
                    EVOLVE,
                    "plan-stored",
                    format!(
                        "from={} to={} digest={} by={caller}",
                        done.plan.from, done.plan.to, done.plan.digest
                    ),
                );
            }
            let mut row = done.to_json();
            row["ran"] = Value::Null;
            // The pre-checks on every database (27w). The plan is stored
            // either way: it is the same everywhere, and the data is not.
            let checks = crate::precheck::precheck(pool, ops.pools.as_ref(), &done.plan)
                .await
                .map_err(|e| e.to_exec_error("the plan was stored and not checked"))?;
            row["prechecks"] = checks.to_json();
            if run && !checks.issues.is_empty() {
                return Err(EvolveRefusal::PreChecks {
                    from: done.plan.from.clone(),
                    to: done.plan.to.clone(),
                    issues: checks.issues,
                }
                .to_exec_error("the plan was stored and not run"));
            }
            if run {
                let ran = run_step_with(
                    pool,
                    pool,
                    "default",
                    &done.plan,
                    caller,
                    &ops.step_context(),
                )
                .await
                .map_err(|e| e.to_exec_error("the plan was stored and not run"))?;
                drop(lock);
                ops.record(
                    caller,
                    EVOLVE,
                    "plan-run",
                    format!(
                        "from={} to={} digest={} database=default by={caller}",
                        done.plan.from, done.plan.to, done.plan.digest
                    ),
                );
                row["ran"] = json!({
                    "database": "default",
                    "statements": ran.statements() as i64,
                    "current": ran == StepRun::Current,
                    "backfills": ran.backfills(),
                });
            }
            Ok(row)
        }
        PLANS => {
            const WHAT: &str = "the stored plans could not be read";
            let all = Plans::on(pool)
                .all()
                .await
                .map_err(|e| e.to_exec_error(WHAT))?;
            let v = Versions::on(pool);
            let from = match text("from") {
                Some(n) => Some(v.resolve(n).await.map_err(|e| e.to_exec_error(WHAT))?),
                None => None,
            };
            let to = match text("to") {
                Some(n) => Some(v.resolve(n).await.map_err(|e| e.to_exec_error(WHAT))?),
                None => None,
            };
            let plans: Vec<Value> = all
                .iter()
                .filter(|p| from.as_deref().is_none_or(|f| p.from == f))
                .filter(|p| to.as_deref().is_none_or(|t| p.to == t))
                .map(StoredPlan::to_json)
                .collect();
            Ok(json!({ "plans": plans }))
        }
        REPLAY => {
            const WHAT: &str = "the tenant was not replayed";
            let tenant = text("tenant").unwrap_or_default();
            let v = Versions::on(pool);
            let Some(pin) = v.pin_of(tenant).await.map_err(|e| e.to_exec_error(WHAT))? else {
                return Err(VersionRefusal::NoTenant {
                    space: tenant.to_string(),
                }
                .to_exec_error(WHAT));
            };
            let to = match text("to") {
                Some(t) => v.resolve(t).await.map_err(|e| e.to_exec_error(WHAT))?,
                None => v
                    .active_version()
                    .await
                    .map_err(|e| e.to_exec_error(WHAT))?
                    .ok_or_else(|| {
                        ExecError::new(
                            ErrorCode::NotFound,
                            format!("{WHAT}: no version is active; name one with `--to`"),
                        )
                    })?,
            };
            let space = ordo_crypto::SpaceId(tenant.to_string());
            let target = ops.pools.resolve(&space).await.map_err(|e| {
                EvolveRefusal::Run {
                    database: tenant.to_string(),
                    from: pin.version.clone().unwrap_or_default(),
                    to: to.clone(),
                    class: "unreachable",
                    why: e.to_string(),
                    transient: true,
                }
                .to_exec_error(WHAT)
            })?;
            let lock = take_lock(pool).await.map_err(|e| e.to_exec_error(WHAT))?;
            let done = replay_with(pool, &target, tenant, &to, caller, &ops.step_context())
                .await
                .map_err(|e| e.to_exec_error(WHAT))?;
            if done.backfilling() {
                // A backfill stopped at its budget: the tenant stays on its
                // version, and the next replay resumes from the cursor.
                drop(lock);
                let mut row = done.to_json();
                row["tenant"] = json!(tenant);
                row["pinned"] = Value::Null;
                return Ok(row);
            }
            // The move's own check before the pin: the tenant's database
            // holds all of the version it is about to serve.
            let program = match v.get(&to).await.map_err(|e| e.to_exec_error(WHAT))? {
                Some(a) => compiled(&a).map_err(|e| e.to_exec_error(WHAT))?,
                None => return Err(VersionRefusal::UnknownVersion(to.clone()).to_exec_error(WHAT)),
            };
            match sa::schema_state(&target, &program.schema).await {
                Ok(sa::SchemaState::Matches { .. }) => {}
                Ok(other) => {
                    return Err(EvolveRefusal::Run {
                        database: tenant.to_string(),
                        from: done.from.clone(),
                        to: to.clone(),
                        class: "still_behind",
                        why: format!("the tenant's database lacks part of `{to}`: {other:?}"),
                        transient: false,
                    }
                    .to_exec_error(WHAT))
                }
                Err(e) => return Err(EvolveRefusal::Store(e).to_exec_error(WHAT)),
            }
            let moved = v
                .pin(tenant, &to, caller, crate::runtime::now_secs())
                .await
                .map_err(|e| {
                    e.to_exec_error(
                        "the tenant's database was replayed and the tenant was not pinned",
                    )
                })?;
            drop(lock);
            ops.record(
                caller,
                REPLAY,
                "replayed",
                format!(
                    "tenant={tenant} from={} to={} steps={} by={caller}",
                    done.from,
                    done.to,
                    done.steps.len()
                ),
            );
            let mut row = done.to_json();
            row["tenant"] = json!(tenant);
            row["pinned"] = json!({ "from": moved.from, "to": moved.to, "epoch": moved.epoch });
            Ok(row)
        }
        ROLLBACK_CHECK => Ok(rollback_check(pool, text("version").unwrap_or_default())
            .await
            .map_err(|e| e.to_exec_error("the version could not be checked"))?
            .to_json()),
        other => Err(ExecError::internal(format!(
            "`{other}` is not an evolve op"
        ))),
    }
}

async fn take_lock(pool: &Pool) -> Result<crate::apply_op::ApplyLock, EvolveRefusal> {
    match crate::apply_op::ApplyLock::take(pool).await {
        Ok(Some(l)) => Ok(l),
        Ok(None) => Err(EvolveRefusal::Busy),
        Err(e) => Err(EvolveRefusal::Store(e)),
    }
}
