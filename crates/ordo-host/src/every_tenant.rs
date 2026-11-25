//! **Every tenant** (`plan-versions.md` increment 11b, part 1;
//! `plan-backfill-dual-writes.md` step B5; §2.5 of `plan-versions.md`).
//!
//! Increment 11a moved one tenant at a time, by hand (`ordo evolve
//! --tenant`). This moves every tenant with a database of its own through
//! the stored path, and brings them back:
//!
//! - **A rollout** moves each tenant through its stored path to the
//!   rollout's target ([`forward_tenant`]). A step that rewrites data by a
//!   per-row mapping is a move with dual writes and a soak
//!   (`crate::dual_writes`); every other step, a seal included, is
//!   replayed and the tenant pinned. A backfill takes `backfill.budget`
//!   chunks per turn, so the rollout's turn stays short and the tenant
//!   says `moving` until its switch.
//! - **An activation's catch-up** replays the stored path on each tenant
//!   database ([`forward_database`]) rather than the apply path, which
//!   refuses what only a stored plan makes (a seal, a fill).
//! - **`ordo rollback` to X** puts every tenant soaking on a move from X
//!   back on X ([`revert_soaks_from`]). It is a pin switch: Y wrote X's
//!   columns too, so every write is kept.
//! - **`ordo rollback --dry-run`** lists each tenant the rollback moves and
//!   what it keeps or loses ([`rollback_tenants`]).
//!
//! # Part 2
//!
//! - A tenant on the deployment's own database moves with `ordo evolve
//!   --tenant`, and the soaks of one step there close together
//!   (`crate::dual_writes`, §4). A rollout still pins such a tenant once
//!   that database holds the target.
//! - A seal on a database several tenants share fences each of them in its
//!   end's transaction ([`seal_fence`]), and the catch-up pins them together.
//! - A rollout's turn moves up to `backfill.concurrency` tenants at once, at
//!   most `backfill.per_database` in one database (`crate::rollouts`).
//! - A grant a move's switch killed stays killed after a revert
//!   (`crate::grant_kills`).

use std::collections::{BTreeMap, BTreeSet};

use ordo_exec::route::PoolResolver;
use ordo_exec::Pool;
use serde_json::{json, Value};

use crate::dual_writes::{MoveContext, Moved};
use crate::evolve::{EvolveRefusal, Plans, StepContext, StoredPlan};
use crate::schema_apply as sa;
use crate::versions::{MovePhase, VersionRefusal, Versions};

/// Where one tenant stands after [`forward_tenant`].
#[derive(Debug)]
pub enum Forward {
    /// No stored step leads from its database's version to the target: the
    /// apply path brings it forward, as before.
    NoPath,
    /// Its database holds the target and passed the gate. The caller pins
    /// the tenant.
    Ready,
    /// A backfill stopped at its budget. The tenant serves its version, and
    /// the next try resumes from the cursor. Says how far it got.
    Backfilling(String),
    /// A move with dual writes switched the tenant: it serves the target in
    /// its soak, or the soak closed at once (`soak` 0).
    Switched(Box<Moved>),
}

/// A backfill's progress, in words, from a step's reports.
fn progress_line(reports: &[Value]) -> String {
    let parts: Vec<String> = reports
        .iter()
        .map(|r| {
            let p = &r["progress"];
            format!(
                "{}.{} {} of {} rows",
                r["entity"].as_str().unwrap_or("?"),
                r["field"].as_str().unwrap_or("?"),
                p["rowsDone"].as_i64().unwrap_or(0),
                p["rowsTotal"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    format!(
        "backfilling ({}); it serves its version, every write keeps both shapes, and the next \
         turn resumes from the cursor",
        if parts.is_empty() {
            "started".to_string()
        } else {
            parts.join(", ")
        }
    )
}

/// **Whether the tenants on the deployment's own database reach `to` by
/// moves with dual writes** (increment 11b part 3): that database records
/// a version from which the stored path to `to` is one step that rewrites
/// data, so a rollout may start with the database still on it. Each
/// tenant's move runs the expand; the soaks there close together.
pub async fn moves_there(control: &Pool, to: &str) -> bool {
    let Ok(ordo_exec::spacedb::TargetVersion::At(from)) =
        ordo_exec::spacedb::recorded_version(control).await
    else {
        return false;
    };
    let one_move = match crate::evolve::path(control, &from, to).await {
        Ok(steps) => steps.len() == 1 && crate::evolve::dual_writes_step(&steps[0]),
        Err(_) => false,
    };
    // A tenant there to move, whose move runs the expand.
    one_move
        && crate::dual_writes::shared_spaces(control)
            .await
            .is_ok_and(|s| s.iter().any(|(_, v)| v == &from))
}

/// Whether a move to `to` on the deployment's own database has copied its
/// rows, so the deployment's own space may serve `to` there.
pub async fn copied_there(control: &Pool, to: &str) -> bool {
    let shared: BTreeSet<String> = match crate::dual_writes::shared_spaces(control).await {
        Ok(s) => s.into_iter().map(|(s, _)| s).collect(),
        Err(_) => return false,
    };
    Versions::on(control).dual_moves().await.is_ok_and(|m| {
        m.iter().any(|m| {
            m.to == to
                && shared.contains(&m.space)
                && matches!(m.phase, MovePhase::Gated | MovePhase::Soaking)
        })
    })
}

/// The refusal for a stored path whose rewriting step is not alone: a
/// move with dual writes switches one step's pin, so the steps around it
/// run first, each on its own.
pub fn runs_alone(
    tenant: &str,
    from: &str,
    to: &str,
    steps: &[StoredPlan],
    i: usize,
) -> EvolveRefusal {
    let stop = if i == 0 { &steps[0].to } else { &steps[i].from };
    EvolveRefusal::Options(format!(
        "the stored path from `{from}` to `{to}` holds {} steps, and the step from `{}` to `{}` \
         rewrites data with live writes beside it, so it runs alone. Replay the tenant to \
         `{stop}` first (`ordo evolve --tenant {tenant} --to {stop}`), then on to `{to}`",
        steps.len(),
        steps[i].from,
        steps[i].to,
    ))
}

/// **Move one tenant with a database of its own toward `to`** through the
/// stored path, as far as this try goes. The caller holds the apply lock.
///
/// - A tenant already in a move with dual writes continues it, when the
///   move goes to `to`.
/// - Its database's recorded version is where the path starts. No stored
///   path from there answers [`Forward::NoPath`].
/// - A path of one rewriting step is a move with dual writes, its soak
///   `soak` seconds long. A rewriting step among others is refused, naming
///   the version to replay to first.
/// - Any other path is replayed on the database, step by step, and checked
///   against `to` before [`Forward::Ready`].
pub async fn forward_tenant(
    cx: &MoveContext<'_>,
    ctx: &StepContext,
    tenant: &str,
    to: &str,
    soak: u64,
    actor: &str,
) -> Result<Forward, EvolveRefusal> {
    let v = Versions::on(cx.control);
    if let Some(m) = v.dual_move(tenant).await? {
        if m.to != to {
            return Err(VersionRefusal::Moving {
                space: m.space,
                from: m.from,
                to: m.to,
                phase: m.phase.as_str().to_string(),
            }
            .into());
        }
        let plan = Plans::on(cx.control)
            .all()
            .await?
            .into_iter()
            .find(|p| p.digest == m.step)
            .ok_or_else(|| {
                EvolveRefusal::Store(format!(
                    "the stored plan {} of tenant `{tenant}`'s move is gone",
                    m.step
                ))
            })?;
        return run_move(cx, ctx, tenant, &plan, soak, actor).await;
    }
    let space = ordo_crypto::SpaceId(tenant.to_string());
    let target = cx
        .pools
        .resolve(&space)
        .await
        .map_err(|e| EvolveRefusal::Run {
            database: tenant.to_string(),
            from: String::new(),
            to: to.to_string(),
            class: "unreachable",
            why: e.to_string(),
            transient: true,
        })?;
    let from = match ordo_exec::spacedb::recorded_version(&target).await {
        Ok(ordo_exec::spacedb::TargetVersion::At(v)) => v,
        Ok(ordo_exec::spacedb::TargetVersion::Unknown) => return Ok(Forward::NoPath),
        Err(e) => {
            return Err(EvolveRefusal::Run {
                database: tenant.to_string(),
                from: String::new(),
                to: to.to_string(),
                class: "unreachable",
                why: e.to_string(),
                transient: true,
            })
        }
    };
    if from == to {
        return Ok(Forward::NoPath);
    }
    let steps = match crate::evolve::path(cx.control, &from, to).await {
        Ok(s) if !s.is_empty() => s,
        Ok(_) | Err(EvolveRefusal::NoPath { .. }) => return Ok(Forward::NoPath),
        Err(e) => return Err(e),
    };
    match steps.iter().position(crate::evolve::dual_writes_step) {
        Some(0) if steps.len() == 1 => run_move(cx, ctx, tenant, &steps[0], soak, actor).await,
        Some(i) => Err(runs_alone(tenant, &from, to, &steps, i)),
        None => {
            let mut ctx = ctx.clone();
            let fences = seal_fence(cx.control, &steps, &[tenant.to_string()], &from, to).await?;
            fence_with(&mut ctx, fences);
            let done = Box::pin(crate::evolve::replay_with(
                cx.control, &target, tenant, to, actor, &ctx,
            ))
            .await?;
            if done.backfilling() {
                let reports = done
                    .steps
                    .last()
                    .map(|(_, r)| r.backfills().to_vec())
                    .unwrap_or_default();
                return Ok(Forward::Backfilling(progress_line(&reports)));
            }
            gate(cx.control, &target, tenant, &done.from, to).await?;
            Ok(Forward::Ready)
        }
    }
}

/// **A seal on the path** (increment 13 under live writes): its end drops
/// the plaintext and raises the pin fence of each tenant on the database to
/// the epoch of the pin that follows, so a node still on the version before
/// writes nothing more there. So a seal is the last step before the pin.
/// On a database several tenants share, every one of them is fenced in the
/// end's transaction and pinned together after it (increment 11b part 2).
/// Empty when the path seals nothing.
async fn seal_fence(
    control: &Pool,
    steps: &[StoredPlan],
    tenants: &[String],
    from: &str,
    to: &str,
) -> Result<Vec<(String, i64)>, EvolveRefusal> {
    let Some(i) = steps.iter().position(crate::evolve::sealing_step) else {
        return Ok(Vec::new());
    };
    let stop = &steps[i].to;
    if i + 1 < steps.len() {
        return Err(EvolveRefusal::Options(format!(
            "the stored path from `{from}` to `{to}` seals a field in the step from `{}` to \
             `{stop}`, and a seal ends with the tenant's pin. Replay the tenant to `{stop}` \
             first (`ordo evolve --tenant <id> --to {stop}`), then on to `{to}`",
            steps[i].from,
        )));
    }
    let v = Versions::on(control);
    let mut out = Vec::new();
    for tenant in tenants {
        let epoch = v.pin_of(tenant).await?.map(|p| p.epoch).unwrap_or(0);
        out.push((tenant.clone(), epoch + 1));
    }
    Ok(out)
}

/// Put [`seal_fence`]'s fences in a step's context.
fn fence_with(ctx: &mut StepContext, fences: Vec<(String, i64)>) {
    let mut it = fences.into_iter();
    ctx.fence = it.next();
    ctx.fences = it.collect();
}

/// One move with dual writes, as far as this try goes.
async fn run_move(
    cx: &MoveContext<'_>,
    ctx: &StepContext,
    tenant: &str,
    plan: &StoredPlan,
    soak: u64,
    actor: &str,
) -> Result<Forward, EvolveRefusal> {
    let moved = Box::pin(crate::dual_writes::run(cx, tenant, plan, ctx, soak, actor)).await?;
    if moved.phase == MovePhase::Backfilling && !moved.closed {
        return Ok(Forward::Backfilling(progress_line(&moved.backfills)));
    }
    Ok(Forward::Switched(Box::new(moved)))
}

/// The move's own check before a pin: the database holds all of `to`.
async fn gate(
    control: &Pool,
    target: &Pool,
    tenant: &str,
    from: &str,
    to: &str,
) -> Result<(), EvolveRefusal> {
    let Some(a) = Versions::on(control).get(to).await? else {
        return Err(VersionRefusal::UnknownVersion(to.to_string()).into());
    };
    let program = crate::evolve::compiled(&a)?;
    match sa::schema_state(target, &program.schema).await {
        Ok(sa::SchemaState::Matches { .. }) => Ok(()),
        Ok(other) => Err(EvolveRefusal::Run {
            database: tenant.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            class: "still_behind",
            why: format!("the tenant's database lacks part of `{to}`: {other:?}"),
            transient: false,
        }),
        Err(e) => Err(EvolveRefusal::Store(e)),
    }
}

/// **Bring one tenant database to `program` by its stored path**, for an
/// activation's catch-up. `None` when no stored path leads there, and the
/// apply path does it as before. A rewriting step is replayed as `ordo
/// evolve --run` runs it on the deployment's own database: `$renames` keeps
/// both columns of a rename current for every version writing there. A
/// seal raises the pin fence of `tenants`, the tenants moving on this
/// database, to the pin the catch-up gives each next ([`seal_fence`]).
pub async fn forward_database(
    control: &Pool,
    target: &Pool,
    label: &str,
    tenants: &[String],
    program: &sa::Program,
    actor: &str,
    ctx: &StepContext,
) -> Option<Result<sa::Forward, sa::ForwardRefused>> {
    let from = match ordo_exec::spacedb::recorded_version(target).await {
        Ok(ordo_exec::spacedb::TargetVersion::At(v)) if v != program.version => v,
        _ => return None,
    };
    let steps = match crate::evolve::path(control, &from, &program.version).await {
        Ok(s) if !s.is_empty() => s,
        _ => return None,
    };
    let refused = |e: EvolveRefusal| sa::ForwardRefused {
        class: e.class(),
        why: e.to_string(),
        transient: e.transient(),
    };
    let mut ctx = ctx.clone();
    match seal_fence(control, &steps, tenants, &from, &program.version).await {
        Ok(f) => fence_with(&mut ctx, f),
        Err(e) => return Some(Err(refused(e))),
    };
    let done = match Box::pin(crate::evolve::replay_with(
        control,
        target,
        label,
        &program.version,
        actor,
        &ctx,
    ))
    .await
    {
        Ok(d) => d,
        Err(e) => return Some(Err(refused(e))),
    };
    if done.backfilling() {
        let reports = done
            .steps
            .last()
            .map(|(_, r)| r.backfills().to_vec())
            .unwrap_or_default();
        return Some(Err(sa::ForwardRefused {
            class: "backfilling",
            why: progress_line(&reports),
            transient: true,
        }));
    }
    if let Err(e) = gate(control, target, label, &from, &program.version).await {
        return Some(Err(refused(e)));
    }
    Some(Ok(sa::Forward::Advanced {
        statements: done
            .steps
            .iter()
            .map(|(_, r)| r.statements())
            .sum::<usize>(),
    }))
}

// ── rollback ─────────────────────────────────────────────────────────────

/// **Put every tenant soaking on a move from `version` back on it**
/// (§2.5): each is a revert in its soak, a pin switch, and every write
/// made since the switch is kept. With `pools`, each tenant database's pin
/// fence rises too (`crate::dual_writes::revert`); without (`ordo rollback
/// --direct`), the pin epoch rises and every node reads it on the pin
/// stream, and both overlays keep both shapes meanwhile. The tenants put
/// back. The caller holds the apply lock.
pub async fn revert_soaks_from(
    control: &Pool,
    pools: Option<&dyn PoolResolver>,
    spaces: Option<&crate::spaces::Spaces>,
    version: &str,
    actor: &str,
) -> Result<Vec<String>, EvolveRefusal> {
    let v = Versions::on(control);
    let soaking: Vec<_> = match v.dual_moves().await {
        Ok(m) => m
            .into_iter()
            .filter(|m| m.phase == MovePhase::Soaking && m.from == version)
            .collect(),
        Err(e) if e.is_missing_table() => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut back = Vec::new();
    for m in soaking {
        match pools {
            Some(pools) => {
                let cx = MoveContext {
                    control,
                    pools,
                    spaces,
                };
                crate::dual_writes::revert(&cx, &m.space, actor).await?;
            }
            None => {
                v.revert_dual_move(&m.space, actor, crate::runtime::now_secs())
                    .await?;
            }
        }
        back.push(m.space);
    }
    Ok(back)
}

/// How many tenants a rollback's dry run names.
pub const DRY_RUN_LISTED: usize = 1_000;

/// **What `ordo rollback --dry-run` to `version` says of each tenant** not
/// on it (§2.5). Reads; changes nothing.
///
/// - One soaking on a move from `version` goes back, every write kept.
/// - One in a tenant build stays: the build moves it.
/// - When `version` is already active (a rollout to another was aborted),
///   no other tenant moves.
/// - Otherwise it moves back. A field its version renamed is read from the
///   old column again. On a tenant's own database that column stopped
///   being written when its soak closed, so what was written since is lost
///   to `version`. On the deployment's own database, `$renames` keeps both
///   columns current until a version drops the `prev` mark.
pub async fn rollback_tenants(control: &Pool, version: &str) -> Result<Vec<Value>, EvolveRefusal> {
    let v = Versions::on(control);
    let moves = match v.dual_moves().await {
        Ok(m) => m,
        Err(e) if e.is_missing_table() => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let active = v.active_version().await?;
    let own: BTreeSet<String> = match ordo_exec::spacedb::all(control).await {
        Ok(a) => a.into_iter().map(|(s, _)| s.0).collect(),
        Err(e) if e.is_missing_table() => BTreeSet::new(),
        Err(e) => return Err(EvolveRefusal::Store(e.to_string())),
    };
    let built: BTreeSet<String> = pairs(
        control,
        "SELECT \"space\", '' FROM \"$tenant_build_members\"",
    )
    .await?
    .into_iter()
    .map(|(s, _)| s)
    .collect();
    let shared_renames = pairs(control, "SELECT \"entity\", \"field\" FROM \"$renames\"").await?;
    let (pins, _) = v.all_pins().await?;
    let mut out = Vec::new();
    let mut renamed_by: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (space, on, _) in pins {
        if on == version || out.len() >= DRY_RUN_LISTED {
            continue;
        }
        let row = |kept: bool, note: String| json!({ "tenant": space, "on": on, "kept": kept, "note": note });
        if let Some(m) = moves
            .iter()
            .find(|m| m.space == space && m.phase == MovePhase::Soaking && m.from == version)
        {
            out.push(row(
                true,
                format!(
                    "in its soak until {}: back on {version}, every write kept",
                    crate::versions::utc_minute(m.soak_until)
                ),
            ));
            continue;
        }
        if built.contains(&space) {
            out.push(row(
                true,
                format!("stays on {on}: it is in a tenant build, which moves it"),
            ));
            continue;
        }
        if active.as_deref() == Some(version) {
            out.push(row(
                true,
                format!(
                    "stays on {on}: {version} is already active, so a rollback moves only the \
                     tenants soaking on a move from it. `ordo evolve --tenant {space} --to \
                     {version}` moves this one"
                ),
            ));
            continue;
        }
        if !renamed_by.contains_key(&on) {
            let renamed = match crate::evolve::path(control, version, &on).await {
                Ok(steps) => steps
                    .iter()
                    .flat_map(|p| renamed_fields(&p.receipt))
                    .collect(),
                Err(_) => Vec::new(),
            };
            renamed_by.insert(on.clone(), renamed);
        }
        let lost: Vec<String> = renamed_by[&on]
            .iter()
            .filter(|r| own.contains(&space) || !shared_renames.contains(*r))
            .map(|(e, f)| {
                if f.is_empty() {
                    e.clone()
                } else {
                    format!("{e}.{f}")
                }
            })
            .collect();
        out.push(if lost.is_empty() {
            row(
                true,
                format!("moves back to {version}; every field it reads is current"),
            )
        } else {
            row(
                false,
                format!(
                    "past its soak, or never in one: moves back to {version}, and what {on} \
                     wrote to {} since its soak closed is not in {version}'s columns",
                    lost.join(", ")
                ),
            )
        });
    }
    Ok(out)
}

/// Each field a stored step renamed, `(entity, field)` by its new name,
/// and each entity it renamed, `(entity, "")`, as `$renames` records them.
fn renamed_fields(receipt: &str) -> Vec<(String, String)> {
    let Ok(r) = serde_json::from_str::<Value>(receipt) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = r["backfills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["code"] == "B4")
        .filter_map(|b| {
            Some((
                b["entity"].as_str()?.to_string(),
                b["field"].as_str()?.to_string(),
            ))
        })
        .collect();
    out.extend(
        crate::evolve::entity_renames(&r)
            .into_iter()
            .map(|(entity, _)| (entity, String::new())),
    );
    out
}

/// Two columns of each row `sql` reads, as a set. A table not made yet
/// reads empty.
async fn pairs(control: &Pool, sql: &str) -> Result<BTreeSet<(String, String)>, EvolveRefusal> {
    match control.rows(sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                Some((it.next()??, it.next().flatten().unwrap_or_default()))
            })
            .collect()),
        Err(e) if e.is_missing_table() => Ok(BTreeSet::new()),
        Err(e) => Err(EvolveRefusal::Store(e.to_string())),
    }
}
