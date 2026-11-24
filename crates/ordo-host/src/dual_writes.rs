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
//! # What waits
//!
//! - A tenant on the deployment's own database shares it with every tenant
//!   there; its move is increment 11b.
//! - A step filled by a backfill function has no per-row mapping for the
//!   write path, so it runs as before, with no dual writes (D16's one-way
//!   step).
//! - Moves driven by a rollout, `ordo rollback`, and the constraints of a
//!   shared database are increment 11b.

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
    }))
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
                .is_some_and(|t| t.columns.contains_key(&bf.column))
        }))
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

    /// The tenant's own database. A tenant on the deployment's own
    /// database shares it with every tenant there, and its move is
    /// increment 11b's.
    async fn target(&self, tenant: &str, from: &str, to: &str) -> Result<Pool, EvolveRefusal> {
        let space = ordo_crypto::SpaceId(tenant.to_string());
        let target = self
            .pools
            .resolve(&space)
            .await
            .map_err(|e| self.err(tenant, from, to, "unreachable", e.to_string(), true))?;
        let (a, b) = (target.config(), self.control.config());
        if a.host == b.host && a.port == b.port && a.db == b.db {
            return Err(self.err(
                tenant,
                from,
                to,
                "shared_database",
                "the tenant's rows are in the deployment's own database, beside other \
                 tenants' that would not write both shapes. A move with dual writes needs a \
                 database of its own (increment 11b moves a shared one). Run the step there \
                 with `ordo evolve --from <x> --to <y> --run`, or give the tenant a database of \
                 its own first"
                    .into(),
                false,
            ));
        }
        Ok(target)
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
    let target = cx.target(tenant, &plan.from, &plan.to).await?;
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
            let Some(overlay) = render(&x, &y, &plan.digest, &backfills, &after) else {
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
        // The switch (§2.1 step 5).
        let at = crate::runtime::now_secs();
        let until = at.saturating_add(soak).max(at);
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
    let target = cx.target(tenant, &m.from, &m.to).await?;
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
    let target = cx.target(tenant, &m.from, &m.to).await?;
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
    let after: Vec<sa::Step> = overlay["after"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str())
        .map(|sql| sa::Step {
            code: "B",
            phase: sa::Phase::Alter,
            sql: format!("{sql}\n"),
            index: None,
            constraint: None,
            table: String::new(),
        })
        .collect();
    if !after.is_empty() {
        sa::execute(&target, &after)
            .await
            .map_err(|f| run_err("execute_failed", f.to_string()))?;
    }
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
    let left: Vec<ordo_exec::orphans::Left> = overlay["orphans"]
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
        .map(|l| format!("{}.{}", l.entity, l.field.as_deref().unwrap_or("")))
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
    let epoch = v.close_dual_move(tenant, actor, at).await?;
    raise_fence(&target, tenant, epoch).await?;
    cx.take(tenant).await;
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
    })
}

/// **Close every soak whose time has passed**, under the apply lock so two
/// nodes never close one twice. Run on the tenant catch-up's tick. Each
/// close that fails is said and tried again on the next tick.
pub async fn close_due(cx: &MoveContext<'_>) -> Vec<String> {
    let now = crate::runtime::now_secs();
    let due: Vec<Move> = match Versions::on(cx.control).dual_moves().await {
        Ok(m) => m
            .into_iter()
            .filter(|m| m.phase == MovePhase::Soaking && m.soak_until <= now)
            .collect(),
        Err(e) if e.is_missing_table() => return Vec::new(),
        Err(e) => {
            eprintln!("soaks: the open moves could not be read ({e})");
            return Vec::new();
        }
    };
    if due.is_empty() {
        return Vec::new();
    }
    let lock = match crate::apply_op::ApplyLock::take(cx.control).await {
        Ok(Some(l)) => l,
        _ => return Vec::new(),
    };
    let mut closed = Vec::new();
    for m in due {
        match close(cx, &m.space, SOAK_ACTOR).await {
            Ok(_) => {
                println!(
                    "soaks: tenant {}'s soak on {} closed; {}'s columns are orphaned",
                    m.space, m.to, m.from
                );
                closed.push(m.space);
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
