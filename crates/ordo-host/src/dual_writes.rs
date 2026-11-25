//! **Dual writes on one tenant** (`plan-versions.md` increment 11a;
//! `plan-backfill-dual-writes.md` steps B2 and B4; owner rulings
//! 2026-10-01, D10, D11, Q1).
//!
//! A stored step that rewrites data (a rename declared with `prev`, a field
//! filled by `--fill`) moves one tenant from version X to version Y while it
//! keeps writing. Nothing it writes is lost, and a revert inside the soak
//! is a pin switch.
//!
//! # The phases (`$moves.phase`)
//!
//! | Phase | Serves | Writes |
//! |---|---|---|
//! | `expanding` | X | both, once the new columns exist |
//! | `backfilling` | X | both: X's ops, then the forward overlay |
//! | `gated` | X | both; verified, ready to switch |
//! | `soaking` | Y | both: Y's ops, then the reverse overlay |
//! | closed | Y | Y only; the row leaves `$moves` |
//!
//! - [`run`] opens the move, runs the expand, the backfill (to its end or
//!   its budget), verify under live writes, and the switch. Run again, it
//!   resumes where it stopped.
//! - [`revert`] moves the pin back in the soak. Every write made on Y is
//!   there on X, because Y also wrote X's columns.
//! - [`close`] ends the soak, early by the deployment owner or when
//!   `soak_until` passes ([`close_due`], on the tenant catch-up's tick).
//!
//! # The overlays
//!
//! Rendered once from the stored receipt when the move opens, and kept in
//! `$moves.overlay`, so every node reads the same ones ([`overlays_of`]).
//! The **forward** overlay sets each new column by the step's forward
//! mapping, and guards what Y requires. The **reverse** overlay sets each
//! renamed field's old column from the new one, and guards what X
//! requires. A write is checked under both versions (D10); a write one of
//! them refuses names both (`ordo_exec::overlay::Refused`).
//!
//! Opening, switching, reverting and closing each raise the tenant's pin
//! epoch in the transaction that changes the move, and the fence in the
//! tenant's database is raised to it at once ([`raise_fence`]). A node that
//! has not read the change yet is refused its next write there, reads the
//! pin and the move again, and writes once more with the right overlay.
//!
//! # The close (Q1)
//!
//! The `NOT NULL` each new column declares lands, the database records Y,
//! each renamed field's old column is recorded in `$orphans`, and the reader
//! grants are made again: a reader loses the old column at the soak's close
//! by a column grant (owner ruling 2026-11-23, Q1;
//! `plan-evolution-federation.md` F2). Then the move leaves `$moves`.
//!
//! # The deployment's own database (increment 11b part 2, §4)
//!
//! A tenant with no database of its own keeps its rows there, beside the
//! deployment's own space and other such tenants. Its move is the same
//! move, with three differences:
//!
//! - The expand and the copy run once for the database, at the first
//!   tenant's move. Later tenants find them done. `$renames` keeps both
//!   columns current for every space there, whatever version it serves.
//! - A step that fills a field records each fill in `$fills` there before
//!   its copy (11b part 4, [`record_fills`]). Every space there, whatever
//!   version it serves, then sets the column by the mapping while its
//!   version does not require the field, as `$renames` keeps a rename.
//! - The soaks of one step there close together ([`close`], [`close_due`]):
//!   when the last is due, and only once no space there serves the version
//!   before ([`still_on`]). The close lands the new shape's constraints,
//!   records the version, orphans the old column and remakes the reader
//!   grants once, for the whole database. A reader of a table there reads
//!   the old column until then (Q1). `ordo evolve --run` of the step there
//!   is refused while such a move is open (`moves_open`).
//!
//! # Uniques and checks the new version adds (11b part 4)
//!
//! In a step with dual writes the version before keeps writing until the
//! soaks close, so a unique or a check Y adds leaves the expand
//! (`schema_apply::Triaged::held`). The move's overlays hold it on every
//! row the moving tenant writes. The copied rows are checked against it
//! before the switch, and every row again at the close; one that breaks it
//! refuses by name (`held_constraint_broken`), naming the rows. A check
//! lands at the close. A unique lands at the switch ([`land_uniques`]):
//! Y's writes that converge on its key need its index.
//! The guard of a unique reads the table for each row written, with no
//! index over the new column yet.
//!
//! # What waits
//!
//! - A step filled by a backfill function has no per-row mapping for the
//!   write path, so it runs as before, with no dual writes (D16's one-way
//!   step).
//! - A rollout drives moves of tenants with a database of their own
//!   (`crate::every_tenant`); a tenant on the deployment's own database
//!   is still pinned by the rollout once that database holds the target.

use std::collections::HashMap;
use std::sync::Arc;

use ordo_exec::overlay::{Guard, MoveOverlay, OverlayEntity};
use ordo_exec::{sql_lit, Pool};
use ordo_schema::ddl::q;
use serde_json::{json, Value};

use crate::deploy::MoveOverlays;
use crate::evolve::{EvolveRefusal, StepContext, StoredPlan};
use crate::schema_apply::{self as sa, BackfillStep, Program};
use crate::versions::{Move, MovePhase, PinChange, Versions};

/// `soak.length`'s default (§2.2): 7 days. 0 is allowed and closes the soak
/// at the switch.
pub const SOAK_DEFAULT_SECS: u64 = crate::evolve::SOAK_DEFAULT_DAYS * 86_400;

/// How long a move's command waits for this node to load the version a
/// tenant was just pinned to.
const LOAD_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// The actor a soak closed by its time is recorded under.
pub const SOAK_ACTOR: &str = "system:soak";

/// Parse `soak.length`: `7d`, `12h`, `30m`, `90s`, or `0`.
pub fn soak_secs(text: &str) -> Result<u64, EvolveRefusal> {
    let t = text.trim();
    if t == "0" {
        return Ok(0);
    }
    crate::share_ops::duration_secs(t).ok_or_else(|| {
        EvolveRefusal::Options(format!(
            "`--soak {t}` is not a length: write `7d`, `12h`, `30m`, or `0` to close the soak \
             at the switch"
        ))
    })
}

// ── the overlays ─────────────────────────────────────────────────────────

/// Render the move's overlays and what its close needs, as `$moves.overlay`
/// keeps them. `None` when a backfill has no per-row mapping (a backfill
/// function): such a step runs with no dual writes.
pub fn render(
    x: &Program,
    y: &Program,
    step: &str,
    backfills: &[BackfillStep],
    after: &[String],
    held: &[sa::Held],
    entities: &[(String, String)],
) -> Option<Value> {
    let mut forward: Vec<(String, Vec<Value>, Vec<Value>)> = Vec::new();
    let mut reverse: Vec<(String, Vec<Value>, Vec<Value>)> = Vec::new();
    let mut orphans = Vec::new();
    fn slot<'a>(
        list: &'a mut Vec<(String, Vec<Value>, Vec<Value>)>,
        entity: &str,
    ) -> &'a mut (String, Vec<Value>, Vec<Value>) {
        if let Some(i) = list.iter().position(|e| e.0 == entity) {
            return &mut list[i];
        }
        list.push((entity.to_string(), Vec::new(), Vec::new()));
        list.last_mut().expect("just pushed")
    }
    for bf in backfills {
        // An entity rename's copy: `$renames` keeps both tables current.
        if bf.copy_from().is_some() {
            continue;
        }
        let sql = bf.sql.as_ref()?;
        let col = q(&bf.column);
        let set = match bf.code {
            // An optional field made required keeps what a row holds.
            "B5" => format!("coalesce({col}, ({sql}))"),
            _ => format!("({sql})"),
        };
        let e = slot(&mut forward, &bf.entity);
        e.1.push(json!([bf.column, set]));
        let required_in_y = y
            .schema
            .native(&bf.entity)
            .and_then(|e| e.field(&bf.field))
            .is_some_and(|f| !f.optional);
        if required_in_y {
            e.2.push(json!({
                "holds": format!("{col} IS NOT NULL"), "field": bf.field, "rule": "is required",
            }));
        }
        if bf.code == "B4" {
            let sa::FillMapping::Expr(from) = &bf.mapping else {
                return None;
            };
            let old = x.schema.native(&bf.entity).and_then(|e| e.field(from))?;
            let r = slot(&mut reverse, &bf.entity);
            if old.optional {
                r.1.push(json!([old.column(), col]));
            } else {
                // The old column keeps its `NOT NULL`, so the version
                // before still loads: an empty new value leaves it as it
                // was, and the guard refuses the write by name.
                r.1.push(json!([
                    old.column(),
                    format!("coalesce({col}, {})", q(&old.column()))
                ]));
                r.2.push(json!({
                    "holds": format!("{col} IS NOT NULL"),
                    "field": from, "rule": "is required",
                }));
            }
            orphans.push(json!({
                "entity": bf.entity, "field": from, "columns": [old.column()],
                "sealed": old.sealed || old.private,
            }));
        }
    }
    // A unique or a check Y adds is held to the close (11b part 4, §4): the
    // host holds it on every row either version writes in the move.
    for h in held {
        let (rule, asks) = h.rule(&y.version);
        let guard = json!({ "holds": h.holds(), "field": asks, "rule": rule });
        slot(&mut forward, &h.entity).2.push(guard.clone());
        slot(&mut reverse, &h.entity).2.push(guard);
    }
    let side = |serving: &str, other: &str, list: Vec<(String, Vec<Value>, Vec<Value>)>| {
        json!({
            "step": step, "serving": serving, "other": other,
            "entities": list.into_iter().map(|(entity, sets, guards)| json!({
                "entity": entity, "sets": sets, "guards": guards,
            })).collect::<Vec<_>>(),
        })
    };
    Some(json!({
        "forward": side(&x.version, &y.version, forward),
        "reverse": side(&y.version, &x.version, reverse),
        "backfills": backfills.iter().map(BackfillStep::to_json).collect::<Vec<_>>(),
        "after": after,
        "orphans": orphans,
        "held": held.iter().map(sa::Held::to_json).collect::<Vec<_>>(),
        // Each entity the step renames, `{entity, from}`: both tables are
        // kept current through `$renames` by every write, and the rename
        // lands at the close (`schema_apply::land_entity_rename`).
        "entities": entities.iter().map(|(entity, from)| json!({
            "entity": entity, "from": from,
        })).collect::<Vec<_>>(),
    }))
}

/// Each entity a move renames, `(new name, old name)`, from its overlays.
fn entities_of(overlay: &Value) -> Vec<(String, String)> {
    overlay["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some((
                e["entity"].as_str()?.to_string(),
                e["from"].as_str()?.to_string(),
            ))
        })
        .collect()
}

fn overlay_of(v: &Value) -> Option<MoveOverlay> {
    Some(MoveOverlay {
        step: v["step"].as_str()?.to_string(),
        serving: v["serving"].as_str()?.to_string(),
        other: v["other"].as_str()?.to_string(),
        entities: v["entities"]
            .as_array()?
            .iter()
            .map(|e| {
                Some(OverlayEntity {
                    entity: e["entity"].as_str()?.to_string(),
                    sets: e["sets"]
                        .as_array()?
                        .iter()
                        .map(|s| Some((s[0].as_str()?.to_string(), s[1].as_str()?.to_string())))
                        .collect::<Option<Vec<_>>>()?,
                    guards: e["guards"]
                        .as_array()?
                        .iter()
                        .map(|g| {
                            Some(Guard {
                                holds: g["holds"].as_str()?.to_string(),
                                field: g["field"].as_str()?.to_string(),
                                rule: g["rule"].as_str()?.to_string(),
                            })
                        })
                        .collect::<Option<Vec<_>>>()?,
                })
            })
            .collect::<Option<Vec<_>>>()?,
    })
}

/// A move's overlays as a node serves them, read from `$moves.overlay`.
pub fn overlays_of(m: &Move) -> Option<MoveOverlays> {
    let v: Value = serde_json::from_str(&m.overlay).ok()?;
    Some(MoveOverlays {
        from: m.from.clone(),
        to: m.to.clone(),
        forward: Arc::new(overlay_of(&v["forward"])?),
        reverse: Arc::new(overlay_of(&v["reverse"])?),
    })
}

/// Every open move's overlays, for a node's whole read.
pub async fn read_all(control: &Pool) -> Result<HashMap<String, MoveOverlays>, EvolveRefusal> {
    let mut out = HashMap::new();
    for m in Versions::on(control).dual_moves().await? {
        match overlays_of(&m) {
            Some(o) => {
                out.insert(m.space.clone(), o);
            }
            None => eprintln!(
                "moves: the overlays of tenant {}'s move from {} to {} do not read; its writes \
                 run none",
                m.space, m.from, m.to
            ),
        }
    }
    Ok(out)
}

/// Whether the database holds every column the move's backfills fill.
async fn expanded(target: &Pool, overlay: &Value) -> Result<bool, EvolveRefusal> {
    let live = ordo_exec::migrate::live_shape(target, target.schema())
        .await
        .map_err(|e| EvolveRefusal::Store(format!("reading the live schema: {e}")))?;
    Ok(overlay["backfills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(BackfillStep::from_json)
        .all(|bf| {
            live.table(&bf.entity)
                .is_some_and(|t| bf.copy_from().is_some() || t.columns.contains_key(&bf.column))
        })
        && entities_of(overlay)
            .iter()
            .all(|(entity, _)| live.table(entity).is_some()))
}

/// Raise the pin fence in the tenant's database to `epoch`: a node still
/// under an older pin, with the older overlays, writes nothing more there.
pub async fn raise_fence(target: &Pool, space: &str, epoch: i64) -> Result<(), EvolveRefusal> {
    target
        .rows(ordo_exec::PIN_FENCE_DDL)
        .await
        .map_err(|e| EvolveRefusal::Store(e.to_string()))?;
    target
        .rows(&format!(
            "INSERT INTO \"$pin_fence\" (\"space\", \"epoch\") VALUES ({}, {epoch}) \
             ON CONFLICT (\"space\") DO UPDATE \
             SET \"epoch\" = GREATEST(\"$pin_fence\".\"epoch\", EXCLUDED.\"epoch\")",
            sql_lit(space)
        ))
        .await
        .map_err(|e| EvolveRefusal::Store(e.to_string()))?;
    Ok(())
}

// ── the move ─────────────────────────────────────────────────────────────

/// Where one tenant's move stands after a command.
#[derive(Debug, Clone)]
pub struct Moved {
    pub tenant: String,
    pub from: String,
    pub to: String,
    pub phase: MovePhase,
    /// Statements the expand sent this time.
    pub statements: usize,
    /// Each backfill's report, as `evolve::run_step` gives it.
    pub backfills: Vec<Value>,
    /// The pin change, when this command moved it.
    pub pinned: Option<PinChange>,
    /// When the soak ends, in seconds.
    pub soak_until: u64,
    /// The soak closed: the move is over.
    pub closed: bool,
    /// What the close recorded in `$orphans`.
    pub orphaned: Vec<String>,
    /// The other tenants whose soaks closed with this one: on the
    /// deployment's own database the soaks of one step close together.
    pub together: Vec<String>,
}

impl Moved {
    pub fn to_json(&self) -> Value {
        json!({
            "tenant": self.tenant,
            "from": self.from,
            "to": self.to,
            "phase": if self.closed { "closed" } else { self.phase.as_str() },
            "statements": self.statements as i64,
            "backfills": self.backfills,
            "pinned": self.pinned.as_ref().map(|p| json!({
                "from": p.from, "to": p.to, "epoch": p.epoch,
            })),
            "soakUntil": (self.soak_until > 0).then_some(self.soak_until),
            "closed": self.closed,
            "orphaned": self.orphaned,
            "closedWith": self.together,
        })
    }
}

/// Everything a move needs from the deployment.
pub struct MoveContext<'a> {
    /// The control database: the registry, `$moves`, the pins.
    pub control: &'a Pool,
    pub pools: &'a dyn ordo_exec::route::PoolResolver,
    /// This node's resolver, to take each change at once. `None` leaves it
    /// to the pin stream.
    pub spaces: Option<&'a crate::spaces::Spaces>,
}

impl MoveContext<'_> {
    fn err(
        &self,
        tenant: &str,
        plan_from: &str,
        plan_to: &str,
        class: &'static str,
        why: String,
        transient: bool,
    ) -> EvolveRefusal {
        EvolveRefusal::Run {
            database: tenant.to_string(),
            from: plan_from.to_string(),
            to: plan_to.to_string(),
            class,
            why,
            transient,
        }
    }

    /// The tenant's database, and whether it is the deployment's own,
    /// shared with every space there (§4, increment 11b part 2).
    async fn target(
        &self,
        tenant: &str,
        from: &str,
        to: &str,
    ) -> Result<(Pool, bool), EvolveRefusal> {
        let space = ordo_crypto::SpaceId(tenant.to_string());
        let target = self
            .pools
            .resolve(&space)
            .await
            .map_err(|e| self.err(tenant, from, to, "unreachable", e.to_string(), true))?;
        let shared = same_database(&target, self.control);
        Ok((target, shared))
    }

    /// Take the tenant's new pin and move on this node now, and wait (up to
    /// [`LOAD_WAIT`]) for this node to serve the version the pin names, so
    /// the command answers once the tenant is served again. The pin stream
    /// loads a version a pin names when it reads the pin.
    async fn take(&self, tenant: &str) {
        let Some(s) = self.spaces else {
            return;
        };
        s.reload_pin(tenant).await;
        let deadline = std::time::Instant::now() + LOAD_WAIT;
        while s.deployment().for_space(tenant).is_err() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

/// **Run one tenant's move** through `plan`, a stored step with
/// backfills: open it, expand, backfill under live writes, verify, switch
/// the pin and start the soak. A budget that stops the backfill answers
/// `backfilling`; run again, it resumes from the cursor. A move already in
/// its soak answers where it stands.
pub async fn run(
    cx: &MoveContext<'_>,
    tenant: &str,
    plan: &StoredPlan,
    ctx: &StepContext,
    soak: u64,
    actor: &str,
) -> Result<Moved, EvolveRefusal> {
    let v = Versions::on(cx.control);
    let (target, shared) = cx.target(tenant, &plan.from, &plan.to).await?;
    let mut statements = 0usize;
    let mut m = match v.dual_move(tenant).await? {
        Some(m) if m.step != plan.digest => {
            return Err(crate::versions::VersionRefusal::Moving {
                space: m.space,
                from: m.from,
                to: m.to,
                phase: m.phase.as_str().to_string(),
            }
            .into())
        }
        Some(m) => m,
        None => {
            // The overlays come from the stored receipt, rendered as this
            // release renders it: the step's expand is what checks the
            // digest, and it runs next.
            let (a, b) = match (v.get(&plan.from).await?, v.get(&plan.to).await?) {
                (Some(a), Some(b)) => (a, b),
                _ => {
                    return Err(
                        crate::versions::VersionRefusal::UnknownVersion(plan.to.clone()).into(),
                    )
                }
            };
            let (x, y) = (crate::evolve::compiled(&a)?, crate::evolve::compiled(&b)?);
            let receipt: Value = serde_json::from_str(&plan.receipt)
                .map_err(|e| EvolveRefusal::Store(format!("the stored receipt: {e}")))?;
            let backfills: Vec<BackfillStep> = receipt["backfills"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(BackfillStep::from_json)
                .collect();
            let after: Vec<String> = receipt["after"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(String::from))
                .collect();
            let held = crate::evolve::held_of(&receipt);
            let entities = crate::evolve::entity_renames(&receipt);
            let Some(overlay) = render(&x, &y, &plan.digest, &backfills, &after, &held, &entities)
            else {
                return Err(cx.err(
                    tenant,
                    &plan.from,
                    &plan.to,
                    "one_way_step",
                    "the step fills a field with a backend function, which has no per-row \
                     mapping for the write path, so it cannot write both shapes"
                        .into(),
                    false,
                ));
            };
            let at = crate::runtime::now_secs();
            let epoch = v
                .open_dual_move(
                    tenant,
                    &plan.from,
                    &plan.to,
                    &plan.digest,
                    &overlay.to_string(),
                    actor,
                    at,
                )
                .await?;
            raise_fence(&target, tenant, epoch).await?;
            cx.take(tenant).await;
            v.dual_move(tenant)
                .await?
                .ok_or_else(|| EvolveRefusal::Store("the move just opened is gone".into()))?
        }
    };
    let overlay: Value = serde_json::from_str(&m.overlay)
        .map_err(|e| EvolveRefusal::Store(format!("the move's overlays: {e}")))?;
    let mut reports = Vec::new();
    if shared && matches!(m.phase, MovePhase::Expanding | MovePhase::Backfilling) {
        // Every space there keeps a fill's column current, whatever version
        // it serves: recorded before the expand and the copy (11b part 4).
        record_fills(&target, &overlay, &plan.from, &plan.to)
            .await
            .map_err(|e| cx.err(tenant, &plan.from, &plan.to, "internal", e, true))?;
    }
    if matches!(m.phase, MovePhase::Expanding | MovePhase::Backfilling) {
        // The expand, once. The database still records X after it. A move
        // stopped after its expand landed and before its phase moved finds
        // every new column there and runs it no more: run again, the drift
        // check would read the old column the expand opened to nulls as X's
        // drift.
        if m.phase == MovePhase::Expanding && !expanded(&target, &overlay).await? {
            if let Some(e) = Box::pin(crate::evolve::expand_step(
                cx.control, &target, tenant, plan, actor,
            ))
            .await?
            {
                statements = e.statements;
            }
        }
        // The plain index each held unique's guard reads, once the expand
        // has made its columns (11b part 5). Run again, it is found there.
        index_held(&target, &overlay)
            .await
            .map_err(|why| cx.err(tenant, &plan.from, &plan.to, "execute_failed", why, true))?;
        if m.phase == MovePhase::Expanding {
            v.note_dual_move(tenant, MovePhase::Backfilling, "").await?;
            m.phase = MovePhase::Backfilling;
        }
        let (Some(b), _) = (v.get(&plan.to).await?, ()) else {
            return Err(crate::versions::VersionRefusal::UnknownVersion(plan.to.clone()).into());
        };
        let y = crate::evolve::compiled(&b)?;
        for r in overlay["backfills"].as_array().into_iter().flatten() {
            let Some(bf) = BackfillStep::from_json(r) else {
                continue;
            };
            let Some(entity) = y.schema.native(&bf.entity) else {
                continue;
            };
            let worker = crate::backfill::Backfill {
                pool: &target,
                step: &plan.digest,
                version: &plan.to,
                fill: &bf,
                entity,
                settings: &ctx.settings,
                functions: ctx.functions.clone(),
                keys: ctx.keys.clone(),
            };
            let fail = |e: crate::backfill::BackfillRefusal| {
                cx.err(
                    tenant,
                    &plan.from,
                    &plan.to,
                    e.class(),
                    e.to_string(),
                    e.transient(),
                )
            };
            let progress = match Box::pin(worker.run()).await {
                Ok(p) => p,
                Err(e) => {
                    let _ = v
                        .note_dual_move(tenant, MovePhase::Backfilling, &e.to_string())
                        .await;
                    return Err(fail(e));
                }
            };
            let mut report = json!({
                "entity": bf.entity, "field": bf.field, "mapping": bf.mapping.text(),
                "audit": bf.audit.as_str(), "progress": progress.to_json(),
            });
            if !progress.ended {
                reports.push(report);
                return Ok(Moved {
                    tenant: tenant.to_string(),
                    from: m.from,
                    to: m.to,
                    phase: MovePhase::Backfilling,
                    statements,
                    backfills: reports,
                    pinned: None,
                    soak_until: 0,
                    closed: false,
                    orphaned: Vec::new(),
                    together: Vec::new(),
                });
            }
            let verified = match Box::pin(worker.verify_live_and_close()).await {
                Ok(v) => v,
                Err(e) => {
                    let _ = v
                        .note_dual_move(tenant, MovePhase::Backfilling, &e.to_string())
                        .await;
                    return Err(fail(e));
                }
            };
            report["verified"] = json!({
                "rows": verified.rows,
                "already": verified.already,
                "spaces": verified.spaces.iter().map(|s| json!({
                    "space": s.space, "rows": s.rows, "content": s.content,
                })).collect::<Vec<_>>(),
            });
            reports.push(report);
        }
        v.note_dual_move(tenant, MovePhase::Gated, "").await?;
        m.phase = MovePhase::Gated;
    }
    let mut pinned = None;
    if m.phase == MovePhase::Gated {
        // A unique or a check Y adds must hold over the copied rows before
        // Y serves them (11b part 4). One that would not is named, with its
        // rows, and the move waits here.
        let fix = format!(
            "run the move again (`ordo evolve --tenant {tenant} --to {}`). The tenant serves \
             `{}` meanwhile",
            plan.to, plan.from
        );
        check_held(&target, &overlay, &m.to, &fix)
            .await
            .map_err(|why| {
                cx.err(
                    tenant,
                    &plan.from,
                    &plan.to,
                    "held_constraint_broken",
                    why,
                    false,
                )
            })?;
        // A unique lands now, before Y serves: Y's writes that converge on
        // its key (`INSERT … ON CONFLICT`) need its index. A check waits for
        // the close.
        land_uniques(&target, &overlay)
            .await
            .map_err(|why| cx.err(tenant, &plan.from, &plan.to, "execute_failed", why, false))?;
        // The switch (§2.1 step 5).
        let at = crate::runtime::now_secs();
        let until = at.saturating_add(soak).max(at);
        // The grants Y refuses die at the switch, and stay dead after a
        // revert (§2.5). Recorded before the pin moves, so a node that reads
        // the new pin reads them too.
        crate::grant_kills::record_at_switch(cx.control, tenant, &m.from, &m.to, at)
            .await
            .map_err(EvolveRefusal::Store)?;
        let change = v.switch_dual_move(tenant, actor, at, until).await?;
        raise_fence(&target, tenant, change.epoch).await?;
        cx.take(tenant).await;
        m.phase = MovePhase::Soaking;
        m.soak_until = until;
        pinned = Some(change);
        if soak == 0 {
            let closed = Box::pin(close(cx, tenant, actor)).await?;
            return Ok(Moved {
                statements,
                backfills: reports,
                pinned,
                ..closed
            });
        }
    }
    Ok(Moved {
        tenant: tenant.to_string(),
        from: m.from,
        to: m.to,
        phase: m.phase,
        statements,
        backfills: reports,
        pinned,
        soak_until: m.soak_until,
        closed: false,
        orphaned: Vec::new(),
        together: Vec::new(),
    })
}

/// **Revert in the soak**: the pin back to the move's `from`. Every write
/// made on `to` also wrote `from`'s columns, so nothing is lost. The move
/// returns to `gated`, both shapes current; [`run`] switches it again.
pub async fn revert(
    cx: &MoveContext<'_>,
    tenant: &str,
    actor: &str,
) -> Result<Moved, EvolveRefusal> {
    let v = Versions::on(cx.control);
    let Some(m) = v.dual_move(tenant).await? else {
        return Err(crate::versions::VersionRefusal::NoMove {
            space: tenant.to_string(),
        }
        .into());
    };
    let (target, _) = cx.target(tenant, &m.from, &m.to).await?;
    let change = v
        .revert_dual_move(tenant, actor, crate::runtime::now_secs())
        .await?;
    raise_fence(&target, tenant, change.epoch).await?;
    cx.take(tenant).await;
    Ok(Moved {
        tenant: tenant.to_string(),
        from: m.from,
        to: m.to,
        phase: MovePhase::Gated,
        statements: 0,
        backfills: Vec::new(),
        pinned: Some(change),
        soak_until: 0,
        closed: false,
        orphaned: Vec::new(),
        together: Vec::new(),
    })
}

/// **The soak's close**: the database becomes the move's `to` alone. The
/// new columns' `NOT NULL` lands, the database records `to`, each renamed
/// field's old column goes to `$orphans`, the reader grants are made again
/// without it (Q1), and the move leaves `$moves`. After it, a revert needs
/// the step's reverse run as a new move.
pub async fn close(
    cx: &MoveContext<'_>,
    tenant: &str,
    actor: &str,
) -> Result<Moved, EvolveRefusal> {
    let v = Versions::on(cx.control);
    let Some(m) = v.dual_move(tenant).await? else {
        return Err(crate::versions::VersionRefusal::NoMove {
            space: tenant.to_string(),
        }
        .into());
    };
    if m.phase != MovePhase::Soaking {
        return Err(crate::versions::VersionRefusal::Moving {
            space: m.space,
            from: m.from,
            to: m.to,
            phase: m.phase.as_str().to_string(),
        }
        .into());
    }
    let (target, shared) = cx.target(tenant, &m.from, &m.to).await?;
    // On the deployment's own database the soaks of one step close
    // together, once no space there serves the version before (§4, Q1).
    let together: Vec<String> = if shared {
        let on_x = still_on(cx.control, &m).await?;
        if !on_x.is_empty() {
            return Err(cx.err(
                tenant,
                &m.from,
                &m.to,
                "soak_shared",
                format!(
                    "the tenant's rows are in the deployment's own database, beside {} still \
                     serving `{}`: {}. The new shape's constraints land, and the soaks of this \
                     step there close together, once every space there serves `{}`. Move them \
                     first (`ordo evolve --tenant <id> --to {}`, or a rollout to `{}`, which \
                     moves the deployment's own space too)",
                    spaces_count(on_x.len()),
                    m.from,
                    on_x.join(", "),
                    m.to,
                    m.to,
                    m.to,
                ),
                true,
            ));
        }
        soaks_together(cx.control, &m).await?
    } else {
        vec![tenant.to_string()]
    };
    let run_err =
        |class: &'static str, why: String| cx.err(tenant, &m.from, &m.to, class, why, false);
    let overlay: Value = serde_json::from_str(&m.overlay)
        .map_err(|e| EvolveRefusal::Store(format!("the move's overlays: {e}")))?;
    let (Some(a), Some(b)) = (v.get(&m.from).await?, v.get(&m.to).await?) else {
        return Err(crate::versions::VersionRefusal::UnknownVersion(m.to.clone()).into());
    };
    let (x, y) = (crate::evolve::compiled(&a)?, crate::evolve::compiled(&b)?);
    // What waited for the close: each new column's `NOT NULL`. Y has
    // written every row since the switch, and the backfill every row before.
    // And each unique or check Y adds, once the rows are checked against
    // it (11b part 4): one that would not land is named, with its rows. A
    // unique the switch landed is not sent again.
    let landed = existing_constraints(&target, &overlay)
        .await
        .map_err(|e| run_err("internal", e))?;
    let after: Vec<sa::Step> = overlay["after"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str())
        .filter(|sql| !landed.iter().any(|n| sql.contains(&q(n))))
        .map(sa::after_step)
        .collect();
    let fix = format!(
        "close the soak again (`ordo evolve --tenant {tenant} --close-soak`), or revert it \
         (`--revert`)"
    );
    check_held(&target, &overlay, &m.to, &fix)
        .await
        .map_err(|why| run_err("held_constraint_broken", why))?;
    if !after.is_empty() {
        sa::execute(&target, &after)
            .await
            .map_err(|f| run_err("execute_failed", f.to_string()))?;
    }
    // Each fill's `NOT NULL` has landed: no space there writes it by its
    // mapping any more.
    let filled: Vec<BackfillStep> = overlay["backfills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(BackfillStep::from_json)
        .collect();
    forget_fills(&target, &filled)
        .await
        .map_err(|e| run_err("internal", format!("$fills: {e}")))?;
    // The guards end with the move: their plain indexes go (11b part 5).
    drop_held_indexes(&target, &overlay)
        .await
        .map_err(|e| run_err("internal", format!("the held indexes: {e}")))?;
    match sa::schema_state(&target, &y.schema).await {
        Ok(sa::SchemaState::Matches { .. }) => {}
        Ok(other) => {
            return Err(run_err(
                "still_behind",
                format!(
                    "the database lacks part of `{}` at the close: {other:?}",
                    m.to
                ),
            ))
        }
        Err(e) => return Err(run_err("internal", e)),
    }
    if ordo_exec::spacedb::recorded_version(&target).await.ok()
        != Some(ordo_exec::spacedb::TargetVersion::At(m.to.clone()))
    {
        sa::record(
            &target,
            &sa::Record {
                kind: ordo_exec::migrate::kind::VERSION,
                from: Some(&m.from),
                to: &m.to,
                actor,
                op: crate::evolve::OP,
                changes: &format!("soak closed, step {}", m.step),
                ddl: "",
                phase: ordo_exec::migrate::phase::APPLIED,
                note: "",
            },
        )
        .await
        .map_err(|e| run_err("internal", e))?;
    }
    // The old columns: kept, never written again, and recorded (§2.7).
    let mut left: Vec<ordo_exec::orphans::Left> = overlay["orphans"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some(ordo_exec::orphans::Left {
                entity: o["entity"].as_str()?.to_string(),
                field: Some(o["field"].as_str()?.to_string()),
                columns: o["columns"]
                    .as_array()?
                    .iter()
                    .filter_map(|c| c.as_str().map(String::from))
                    .collect(),
                sealed: o["sealed"].as_bool().unwrap_or(true),
            })
        })
        .collect();
    // Each entity rename lands: the references move to the new table, the
    // rename leaves `$renames`, and the old table is an orphan.
    for (entity, from) in entities_of(&overlay) {
        let landed = sa::land_entity_rename(&target, &y.schema, &entity, &from)
            .await
            .map_err(|e| run_err("execute_failed", e))?;
        if landed {
            left.push(ordo_exec::orphans::Left {
                entity: from,
                field: None,
                columns: Vec::new(),
                sealed: false,
            });
        }
    }
    let at = crate::runtime::now_secs();
    ordo_exec::orphans::record(&target, &left, Some(&m.from), &m.to, actor, at)
        .await
        .map_err(|e| run_err("internal", format!("$orphans: {e}")))?;
    // Each orphaned old column leaves `$renames`: no version writes it
    // again, and Y's writes stop keeping it current.
    for l in &left {
        let Some(field) = &l.field else { continue };
        match target
            .rows(&format!(
                "DELETE FROM \"$renames\" WHERE \"entity\" = {} AND \"from\" = {}",
                sql_lit(&l.entity),
                sql_lit(field)
            ))
            .await
        {
            Ok(_) => {}
            Err(e) if e.is_missing_table() => {}
            Err(e) => return Err(run_err("internal", format!("$renames: {e}"))),
        }
    }
    let mut orphaned: Vec<String> = left
        .iter()
        .map(|l| match &l.field {
            Some(f) => format!("{}.{f}", l.entity),
            None => l.entity.clone(),
        })
        .collect();
    orphaned.extend(
        sa::record_orphans(&target, &x.schema, &y, Some(&m.from), actor)
            .await
            .map_err(|e| run_err("internal", e))?,
    );
    // Q1: a reader loses the old column now, by a column grant.
    ordo_exec::peer::grant_readers(&target, &y.schema)
        .await
        .map_err(|e| run_err("reader_grants", e.to_string()))?;
    for t in &together {
        let epoch = v.close_dual_move(t, actor, at).await?;
        raise_fence(&target, t, epoch).await?;
        cx.take(t).await;
    }
    Ok(Moved {
        tenant: tenant.to_string(),
        from: m.from,
        to: m.to,
        phase: MovePhase::Soaking,
        statements: 0,
        backfills: Vec::new(),
        pinned: None,
        soak_until: m.soak_until,
        closed: true,
        orphaned,
        together: together.into_iter().filter(|t| t != tenant).collect(),
    })
}

fn spaces_count(n: usize) -> String {
    if n == 1 {
        "1 space".to_string()
    } else {
        format!("{n} spaces")
    }
}

/// Whether two pools reach one database.
pub fn same_database(a: &Pool, b: &Pool) -> bool {
    let (a, b) = (a.config(), b.config());
    a.host == b.host && a.port == b.port && a.db == b.db
}

/// **A fill on the deployment's own database keeps every space current**
/// (11b part 4). Each fill of the move's step is recorded in that
/// database's `$fills`, under `"$renames"` in `EXCLUSIVE` mode, before the
/// expand and the copy: every write takes that lock in `ROW SHARE` mode
/// first, so a write that read the table before has committed when the copy
/// starts, and every write after reads the fill and, while its version
/// does not require the field, sets the column by the mapping
/// (`ordo_exec::overlay::MoveOverlay::for_database`). The copy runs once
/// for the database. Run again, it records the same rows.
async fn record_fills(target: &Pool, overlay: &Value, from: &str, to: &str) -> Result<(), String> {
    let mut sets: Vec<(String, String, String, String)> = Vec::new();
    for e in overlay["forward"]["entities"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let entity = e["entity"].as_str().unwrap_or_default();
        for s in e["sets"].as_array().into_iter().flatten() {
            let (Some(column), Some(sql)) = (s[0].as_str(), s[1].as_str()) else {
                continue;
            };
            // A rename's copy is `$renames`' already.
            let fill = overlay["backfills"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(BackfillStep::from_json)
                .find(|b| b.entity == entity && b.column == column && b.code != "B4");
            if let Some(b) = fill {
                sets.push((
                    entity.to_string(),
                    b.field,
                    column.to_string(),
                    sql.to_string(),
                ));
            }
        }
    }
    if sets.is_empty() {
        return Ok(());
    }
    let e = |x: ordo_exec::PoolError| x.to_string();
    target
        .rows(ordo_exec::overlay::RENAMES_DDL)
        .await
        .map_err(e)?;
    target
        .rows(ordo_exec::overlay::FILLS_DDL)
        .await
        .map_err(e)?;
    let tx = target
        .begin_for(ordo_exec::OpClass::AddressedWrite)
        .await
        .map_err(e)?;
    tx.rows("LOCK TABLE \"$renames\" IN EXCLUSIVE MODE")
        .await
        .map_err(e)?;
    for (entity, field, column, sql) in &sets {
        tx.rows(&format!(
            "INSERT INTO \"$fills\" (\"entity\", \"field\", \"column\", \"sql\", \"from\", \"to\") \
             VALUES ({}, {}, {}, {}, {}, {}) ON CONFLICT (\"entity\", \"field\") DO UPDATE SET \
             \"column\" = EXCLUDED.\"column\", \"sql\" = EXCLUDED.\"sql\", \
             \"from\" = EXCLUDED.\"from\", \"to\" = EXCLUDED.\"to\"",
            sql_lit(entity),
            sql_lit(field),
            sql_lit(column),
            sql_lit(sql),
            sql_lit(from),
            sql_lit(to),
        ))
        .await
        .map_err(e)?;
    }
    tx.commit().await.map_err(e)
}

/// **A fill's record leaves `$fills`** once its `NOT NULL` has landed: at
/// the close of the step's soaks on a shared database, or at the end of the
/// step run there with `ordo evolve --run`. Nothing when none was recorded.
pub(crate) async fn forget_fills(
    target: &Pool,
    backfills: &[BackfillStep],
) -> Result<(), ordo_exec::PoolError> {
    for b in backfills.iter().filter(|b| b.code != "B4") {
        match target
            .rows(&format!(
                "DELETE FROM \"$fills\" WHERE \"entity\" = {} AND \"field\" = {}",
                sql_lit(&b.entity),
                sql_lit(&b.field)
            ))
            .await
        {
            Ok(_) => {}
            Err(e) if e.is_missing_table() => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// What the move holds (`$moves.overlay`'s `held`, 11b part 4).
fn held_in(overlay: &Value) -> Vec<sa::Held> {
    overlay["held"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(sa::Held::from_json)
        .collect()
}

/// **The plain index each held unique's guard reads** (11b part 5): over
/// the space and the unique's columns, built `CONCURRENTLY` so writes go on.
/// Without it the guard reads the whole table for each row written. Made
/// once the expand has made the columns; found there on a run again or by a
/// later tenant's move on the same database.
async fn index_held(target: &Pool, overlay: &Value) -> Result<(), String> {
    let steps: Vec<sa::Step> = held_in(overlay)
        .iter()
        .filter_map(|h| {
            let (name, sql) = h.index()?;
            Some(sa::Step {
                code: "C1",
                phase: sa::Phase::ConcurrentIndex,
                sql,
                index: Some(name),
                constraint: None,
                table: h.entity.clone(),
            })
        })
        .collect();
    if steps.is_empty() {
        return Ok(());
    }
    sa::execute(target, &steps)
        .await
        .map(|_| ())
        .map_err(|f| f.to_string())
}

/// The held indexes go at the close: the unique's own index, landed at the
/// switch, serves from then on.
async fn drop_held_indexes(target: &Pool, overlay: &Value) -> Result<(), ordo_exec::PoolError> {
    for h in held_in(overlay) {
        if let Some((name, _)) = h.index() {
            target
                .rows_in_session(&[&format!("DROP INDEX CONCURRENTLY IF EXISTS {}", q(&name))])
                .await?;
        }
    }
    Ok(())
}

/// The rows checked against each unique or check the move holds.
async fn check_held(target: &Pool, overlay: &Value, to: &str, fix: &str) -> Result<(), String> {
    crate::evolve::check_held(target, &held_in(overlay), to, fix).await
}

/// The held constraints the database already has, by name.
async fn existing_constraints(target: &Pool, overlay: &Value) -> Result<Vec<String>, String> {
    let names: Vec<String> = held_in(overlay).into_iter().map(|h| h.name).collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let list = names
        .iter()
        .map(|n| sql_lit(n))
        .collect::<Vec<_>>()
        .join(", ");
    let rows = target
        .rows(&format!(
            "SELECT conname::text FROM pg_constraint WHERE conname IN ({list})"
        ))
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect())
}

/// **A unique lands at the switch** (11b part 4), its rows checked first:
/// the version after converges on its key (`INSERT … ON CONFLICT`), which
/// needs its index. Once there, a later switch on the same database finds
/// it and sends nothing.
async fn land_uniques(target: &Pool, overlay: &Value) -> Result<(), String> {
    let landed = existing_constraints(target, overlay).await?;
    let steps: Vec<sa::Step> = held_in(overlay)
        .into_iter()
        .filter(|h| matches!(h.kind, sa::HeldKind::Unique { .. }) && !landed.contains(&h.name))
        .flat_map(|h| {
            overlay["after"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str())
                .filter(|sql| sql.contains(&q(&h.name)))
                .map(sa::after_step)
                .collect::<Vec<_>>()
        })
        .collect();
    if steps.is_empty() {
        return Ok(());
    }
    sa::execute(target, &steps)
        .await
        .map(|_| ())
        .map_err(|f| f.to_string())
}

/// The tenants in the deployment's own database, with their versions:
/// every pinned tenant with no database of its own.
pub(crate) async fn shared_spaces(control: &Pool) -> Result<Vec<(String, String)>, EvolveRefusal> {
    let own: std::collections::BTreeSet<String> = match ordo_exec::spacedb::all(control).await {
        Ok(a) => a.into_iter().map(|(s, _)| s.0).collect(),
        Err(e) if e.is_missing_table() => Default::default(),
        Err(e) => return Err(EvolveRefusal::Store(e.to_string())),
    };
    let (pins, _) = Versions::on(control).all_pins().await?;
    Ok(pins
        .into_iter()
        .filter(|(s, _, _)| !own.contains(s))
        .map(|(s, v, _)| (s, v))
        .collect())
}

/// Every soak of `m`'s step on the deployment's own database: they close
/// together.
async fn soaks_together(control: &Pool, m: &Move) -> Result<Vec<String>, EvolveRefusal> {
    let shared: std::collections::BTreeSet<String> = shared_spaces(control)
        .await?
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    let mut out: Vec<String> = Versions::on(control)
        .dual_moves()
        .await?
        .into_iter()
        .filter(|o| o.step == m.step && o.phase == MovePhase::Soaking)
        .filter(|o| o.space == m.space || shared.contains(&o.space))
        .map(|o| o.space)
        .collect();
    if !out.contains(&m.space) {
        out.push(m.space.clone());
    }
    out.sort();
    Ok(out)
}

/// **Who on the deployment's own database still writes `m.from`**: a
/// tenant pinned there outside a soak of this step, and the deployment's
/// own space while it serves it.
pub async fn still_on(control: &Pool, m: &Move) -> Result<Vec<String>, EvolveRefusal> {
    let v = Versions::on(control);
    let soaking: std::collections::BTreeSet<String> = v
        .dual_moves()
        .await?
        .into_iter()
        .filter(|o| o.step == m.step && o.phase == MovePhase::Soaking)
        .map(|o| o.space)
        .collect();
    let mut out: Vec<String> = shared_spaces(control)
        .await?
        .into_iter()
        .filter(|(s, version)| *version == m.from && !soaking.contains(s))
        .map(|(s, _)| s)
        .collect();
    let own = match v.own_pin().await? {
        Some((space, version, _)) => (version == m.from).then_some(space),
        None => (v.active_version().await?.as_deref() == Some(m.from.as_str()))
            .then(|| "the deployment's own space".to_string()),
    };
    out.extend(own);
    out.sort();
    out.dedup();
    Ok(out)
}

/// **Close every soak whose time has passed**, under the apply lock so two
/// nodes never close one twice. Run on the tenant catch-up's tick. Each
/// close that fails is said and tried again on the next tick.
pub async fn close_due(cx: &MoveContext<'_>) -> Vec<String> {
    let now = crate::runtime::now_secs();
    let open: Vec<Move> = match Versions::on(cx.control).dual_moves().await {
        Ok(m) => m
            .into_iter()
            .filter(|m| m.phase == MovePhase::Soaking)
            .collect(),
        Err(e) if e.is_missing_table() => return Vec::new(),
        Err(e) => {
            eprintln!("soaks: the open moves could not be read ({e})");
            return Vec::new();
        }
    };
    if !open.iter().any(|m| m.soak_until <= now) {
        return Vec::new();
    }
    // On the deployment's own database the soaks of one step close
    // together, when the last is due (Q1): a reader of a table there reads
    // the old column until then.
    let shared: std::collections::BTreeSet<String> = match shared_spaces(cx.control).await {
        Ok(s) => s.into_iter().map(|(s, _)| s).collect(),
        Err(e) => {
            eprintln!(
                "soaks: the tenants on the deployment's own database could not be read ({e})"
            );
            return Vec::new();
        }
    };
    let due: Vec<&Move> = open
        .iter()
        .filter(|m| m.soak_until <= now)
        .filter(|m| {
            !shared.contains(&m.space)
                || open
                    .iter()
                    .filter(|o| o.step == m.step && shared.contains(&o.space))
                    .all(|o| o.soak_until <= now)
        })
        .collect();
    if due.is_empty() {
        return Vec::new();
    }
    let lock = match crate::apply_op::ApplyLock::take(cx.control).await {
        Ok(Some(l)) => l,
        _ => return Vec::new(),
    };
    let mut closed: Vec<String> = Vec::new();
    for m in due {
        if closed.contains(&m.space) {
            continue;
        }
        match close(cx, &m.space, SOAK_ACTOR).await {
            Ok(done) => {
                println!(
                    "soaks: tenant {}'s soak on {} closed{}; {}'s columns are orphaned",
                    m.space,
                    m.to,
                    if done.together.is_empty() {
                        String::new()
                    } else {
                        format!(", with {}", done.together.join(", "))
                    },
                    m.from
                );
                closed.push(m.space.clone());
                closed.extend(done.together);
            }
            Err(e) => eprintln!(
                "soaks: tenant {}'s soak did not close ({e}); tried again",
                m.space
            ),
        }
    }
    drop(lock);
    closed
}
