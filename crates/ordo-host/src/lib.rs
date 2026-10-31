//! Stage 7 — the wiring crate: `ordo-api`'s traits fulfilled by the real
//! compiler and the conformance execution tier.
//!
//! `ordo-api` deliberately depends on no sibling crate; this crate is the
//! other half of that bargain. It derives every [`OpSpec`] from the
//! registered program (never hand-written — the manifest IS the compiler's
//! output), executes reads through the bound-and-planned pipeline and
//! writes through the Stage-5 write protocol, and answers the door's
//! access question from platform membership.
//!
//! Both paths run on F5's executor. Reads: pooled `tokio-postgres`, venue
//! gate at the plan, canonical decode by declared output type. Writes: the
//! Stage-5 protocol under ONE transaction — `FOR UPDATE` on the addressed
//! row, preconditions and mutations sharing a snapshot, commit on success
//! and abort on any refusal (AA-004, closed).
//!
//! Reads page by keyset over the plan's own total order: the executor
//! mints the cursor keys, the API layer MACs them into an opaque token
//! bound to space, op, and principal. A cursor is minted only when the
//! page came back full — the alternative is a second count query to say
//! "no more", which pages nobody asked for.
//!
//! Honestly absent: tokens are the conformance `member:<id>` form rather
//! than a verified credential (`ordo-identity`'s claim engine wires in
//! when sessions exist).
//!
//! # The deployment, assembled
//!
//! Four things a deployment has that a wiring crate did not:
//!
//! - [`deploy`] — the **swap seam**. One `Loaded` version behind one lock,
//!   holding the fixture, the executor, the specs and the shape report
//!   together so a hot swap moves all of them or none, and typed so that
//!   only a gated candidate can be installed.
//! - [`shape`] — the **load gate**. Every registered op's inferred shape is
//!   verified against what Postgres reports for it, and a mismatch refuses
//!   the boot. Nothing here can be served without a [`shape::ShapeReport`],
//!   and the only way to get one is to pass.
//! - [`serve`] — the deployment's own HTTP surface: `/health/live`,
//!   `/health/ready`, an access-controlled `/health/deep`, the scoped
//!   catalog in both renderings, and access-controlled metrics. A second
//!   router merged alongside `ordo-api`'s generated one, for the reasons in
//!   that module's doc.
//! - [`invalidation`] — the LISTEN half of `?live=1`, so a subscriber on
//!   one node sees another node's commits.
//! - [`metrics`] — the counter substrate the two mandatory security alerts
//!   need, with its limits written down.

pub mod access_template;
pub mod account_removal;
pub mod admission;
pub mod apply_op;
pub mod attest;
pub mod attest_ops;
pub mod attestdrain;
pub mod auth;
pub mod blob;
pub mod boot_record;
pub mod bucket_ops;
pub mod ceremonies;
pub mod consent;
pub mod console_keys;
pub mod contact_email;
pub mod credential_ops;
pub mod credentials;
pub mod decrypt_seen;
pub mod delegable;
pub mod delegation;
pub mod deploy;
pub mod deploy_ops;
pub mod deployment_access;
pub mod directory;
pub mod email_code;
pub mod enrolment;
pub mod exit_ops;
pub mod federation;
pub mod fleet;
pub mod grants;
pub mod guess_limits;
pub mod identity_keys;
pub mod identity_ops;
pub mod identity_removal;
pub mod invalidation;
pub mod invite_ops;
pub mod layered_limits;
pub mod link_password;
pub mod mail;
pub mod member_ops;
pub mod metrics;
pub mod node_status;
pub mod observed_cost;
pub mod oidc;
pub mod own_download;
pub mod passkeys;
pub mod pause;
pub mod people_ops;
pub mod pgsync;
pub mod platform_ops;
pub mod presence;
pub mod principal_directory;
pub mod profile_ops;
pub mod provision;
pub mod proxy;
pub mod public;
pub mod quotas;
pub mod reap_op;
pub mod recovery;
pub mod recovery_codes;
pub mod recovery_hold;
pub mod recovery_settings;
mod refine_call;
pub mod registrations;
pub mod release_ops;
pub mod renewal;
pub mod replay;
pub mod routines;
pub mod runtime;
pub mod schema_apply;
pub mod secdrain;
pub mod second_factor_store;
pub mod second_factors;
pub mod security_key;
pub mod security_record;
pub mod self_serve;
pub mod serve;
pub mod session_policy;
pub mod sessions;
pub mod settings;
pub mod shape;
pub mod share_accept;
pub mod share_links;
pub mod share_ops;
pub mod spaces;
pub mod supervisor;
pub mod tenant_forward;
pub mod tenant_identities;
pub mod tenant_ops;
pub mod tenant_origins;
pub mod tenant_providers;
pub mod tenant_setup;
pub mod tenants;
pub mod tickets;
pub mod workers;

use std::sync::Arc;

use futures::future::BoxFuture;
use ordo_api::{
    AccessChecker, AccessDecision, BoundParams, ErrorCode, ExecError, ExecOutcome, ExecResult,
    FieldSpec, NotSyncable, OpCost, OpExecutor, OpKind, OpSpec, ParamSpec, ParamTy, RefineTier,
    ReplayLease, Syncability,
};
use ordo_conformance::{Fixture, Pg};
use ordo_syntax::{OpBody, ScalarTy, WriteStmt};
use sha2::{Digest, Sha256};

/// A space that could not be resolved, in the executor's vocabulary.
///
/// Four refusals with three very different remedies, which is the whole
/// reason this is a function rather than one `internal` for all of them
/// (18d R-F2).
///
/// - [`spaces::SpaceRefusal::Unroutable`] is a **dependency that is
///   down**: the tenant's database could not be reached. `503`, and the
///   identical call lands when it comes back. The reason is **not**
///   carried to the caller, per §9.1 R-C4.4: a dial error names a host, a
///   port and often a user, which is deployment topology a space owner
///   has no business reading, and for a tenant on their own cluster it is
///   their own infrastructure described back to them by a third party.
///   The deployment owner reads it in `/health/ready` and in the log.
///   **One exception**: a resolve that reached the database and found
///   this deployment's pool full is `server_busy` (25z). Nothing is down.
/// - [`spaces::SpaceRefusal::SchemaBehind`] is **the app changing under the
///   tenant** (25i): the tenant's data is being brought to the current
///   version of the app, and the same request will land when that is done.
///   `503` like the unreachable case, and told apart from it, because the
///   remedy differs: nothing is down. The caller is not told which
///   databases or versions are involved, for the same reason as above. The
///   deployment owner reads them in `/health/ready` and in the log.
/// - [`spaces::SpaceRefusal::Rewound`] is the **deployment disagreeing
///   with itself** and needs a human to record a restore. No amount of
///   retrying fixes it, so it keeps `internal`. Telling a client to retry
///   would be telling it to retry forever.
/// - [`spaces::SpaceRefusal::NoCustody`] is the same shape: a space the
///   door authorized and the platform cannot serve, which is a repair
///   rather than a wait.
fn space_refusal(e: spaces::SpaceRefusal) -> ExecError {
    match e {
        // The database answered and this deployment's pool for it was full
        // (25z). Reachable, so not `space_unavailable`.
        e if e.is_busy() => ExecError::server_busy(),
        spaces::SpaceRefusal::Unroutable { space, .. } => ExecError::new(
            ErrorCode::SpaceUnavailable,
            format!(
                "space `{space}` is served from a database this deployment cannot reach just \
                 now. Nothing was written and the same request will land when it is back; if \
                 it persists, the deployment owner has the reason"
            ),
        ),
        spaces::SpaceRefusal::SchemaBehind { space, .. } => ExecError::new(
            ErrorCode::SpaceUnavailable,
            format!(
                "space `{space}` is closed for a moment while its data is brought to the \
                 current version of the app. Nothing was written and the same request will \
                 land when that is done; if it persists, the deployment owner has the reason"
            ),
        ),
        other => ExecError::internal(other.to_string()),
    }
}

/// The space a deployment falls back to as **its own** when nothing
/// configured one.
///
/// The deployment's own space answers the narrow question D0 found: which
/// space the *reserved* surfaces authorize against
/// ([`serve::PlatformState::space`]). It is no longer "the space this host
/// serves", which is every space the process has custody and a membership
/// for, resolved per request by [`spaces::Spaces`]. `/metrics` names
/// principals across every space this process served and `/health/deep`
/// names the key provider and the database, so each is a deployment-wide
/// disclosure with exactly one space in it, and it is not one the caller
/// may name.
///
/// The conformance fixture's space is the default because the fixture is
/// what an unconfigured deployment is serving. A deployment states its own
/// with `ORDO_DEPLOYMENT_SPACE` ([`deployment_space`]) or by passing one to
/// [`serve::PlatformState::new`] directly.
pub const DEFAULT_SPACE: &str = ordo_conformance::FIXTURE_SPACE;

/// The deployment's own space, from the environment.
///
/// Configuration rather than a constant, because a binary that hard-codes
/// which tenant administers it is a binary that can only ever be deployed
/// once. Read at the point a deployment is assembled, never on the request
/// path: the request path has the door's binding and has no business
/// consulting this at all.
///
/// A set-but-unusable value is refused rather than defaulted. Falling back
/// would leave the process authorizing `/metrics` and `/health/deep`
/// against a tenant nobody chose while the operator believes it is
/// authorizing against the one they named, and that disagreement is
/// invisible from both ends.
pub fn deployment_space() -> Result<String, String> {
    match std::env::var("ORDO_DEPLOYMENT_SPACE") {
        Err(_) => Ok(DEFAULT_SPACE.to_string()),
        Ok(s) if ordo_api::well_formed_space(&s) => Ok(s),
        Ok(s) => Err(format!(
            "ORDO_DEPLOYMENT_SPACE is `{s}`, which is not a space id this door can \
             bind, so refusing to fall back to `{DEFAULT_SPACE}`"
        )),
    }
}

/// Derive the full spec set from the fixture's registered ops. Every field
/// of every spec is compiler output: kind from the op body, params from
/// the declarations, output shape from binding the pipeline, the version
/// hash from the registered source's stable debug form.
pub fn build_specs(f: &Fixture) -> Vec<OpSpec> {
    let mut names: Vec<&String> = f.registry.names().collect();
    names.sort();
    let mut specs = Vec::new();
    for name in names {
        let Some(op) = f.registry.get(name) else {
            continue;
        };
        let params: Vec<ParamSpec> = op
            .params
            .iter()
            .map(|p| ParamSpec {
                name: p.name.name.clone(),
                // Not a fallback. `ordo_sema` refuses a parameter whose
                // type has no wire form before an op can be registered, so
                // a `None` here means that refusal regressed and an op is
                // one line away from binding bytes as a string.
                ty: param_ty(p).unwrap_or_else(|| {
                    panic!(
                        "op `{name}` declares `${}: {}`, which is not a parameter type — \
                         `ordo_sema` refuses it at compile time, so reaching the door \
                         means that refusal is gone",
                        p.name.name,
                        p.ty.as_str()
                    )
                }),
                required: p.default.is_none(),
                bounds: param_bounds(op, p),
            })
            .collect();
        let (kind, mode_label, output_fields, aggregate_only, cost, sync, refinable) =
            match &op.body {
                OpBody::Write(stmts) => {
                    let all_del = stmts.iter().all(|s| matches!(s, WriteStmt::Del { .. }));
                    // §6 rule 1: the wire reports the op-level UNION of the
                    // per-statement labels; the artifact and the catalog carry
                    // the list. Both come from `ordo_plan::writes` — rule 8's
                    // own classifier — because a second reading of the braces
                    // here is how the label and the behaviour drift apart, and
                    // it had: this crate answered `converges` for every
                    // conditioned `set`, so a bulk write over a range and an
                    // addressed write over one row printed the same word.
                    let kind = if all_del {
                        OpKind::Delete
                    } else {
                        OpKind::Write
                    };
                    (
                        kind,
                        Some(ordo_plan::writes::op_label(&f.schema, stmts)),
                        Vec::new(),
                        false,
                        // The cost gate classifies READ plans; a write op has no
                        // read plan, and no refinement is legal on one either
                        // (the door refuses `$refine` on a write outright), so
                        // this is the never-consulted default rather than an
                        // invented "write" class.
                        OpCost::default(),
                        Syncability::NotSyncable(NotSyncable::NotARead),
                        // A write has no rows to refine, and the door refuses
                        // `$refine` on a non-read before the tier is read.
                        RefineTier::None,
                    )
                }
                OpBody::Read(pl) => {
                    // The output shape AND the cost facts come from actually
                    // binding the op — the same authority every read request
                    // goes through, and the same authority the artifact and the
                    // catalog print.
                    let (fields, cost, sync, refinable) = match ordo_sema::bind_op_generic(
                        &f.schema,
                        &f.registry,
                        name,
                        ordo_sema::Clock::CORPUS,
                    ) {
                        Ok(mut q) => {
                            // The fifth site that plans or describes a read, and the
                            // one the shape gate caught missing (C4e). `build_specs`
                            // feeds one bound query to BOTH `manifest_fields` and
                            // `syncability`, so attaching here keeps the door's field
                            // list and its sync key agreeing with the descriptor the
                            // artifact emits. Without it the descriptor claimed `$id`
                            // and the door's spec did not, which is the silent
                            // disagreement `ui-gen.md` §1 says can never happen.
                            let _ = ordo_plan::artifact::project_row_key(&mut q);
                            let mut fields = manifest_fields(f, &q);
                            if let Some(list) = count_pair(f, name) {
                                fields = count_fields(f, &q, &list, fields);
                            }
                            (
                                fields,
                                registered_cost(f, &q, op.flags.unbounded),
                                syncability(&q),
                                // Every compiled read is refinable, with no off
                                // switch (owner, 2026-10-05: "this should be a
                                // standard feature"). The manifest is the op's
                                // own outputs, so a refinement only narrows.
                                //
                                // A public read is `None`: declared parameters
                                // only, because each refinement would be a cache
                                // key (`design-public-path.md`, ruled ask 1).
                                if op.flags.public {
                                    RefineTier::None
                                } else {
                                    RefineTier::Basic
                                },
                            )
                        }
                        // An op that will not bind has no manifest and no proven
                        // anything; `OpCost::default()` is the conservative
                        // reading of that (see its docs), not a cheap guess.
                        // It has no anchor either, for the same reason.
                        // And no manifest means nothing a field could be
                        // validated against, so it is not refinable.
                        Err(_) => (
                            Vec::new(),
                            OpCost::default(),
                            Syncability::NotSyncable(NotSyncable::NoAnchor),
                            RefineTier::None,
                        ),
                    };
                    // Keyless-group tail = the op reduces rows to a measure.
                    let agg = pl
                        .steps
                        .iter()
                        .any(|s| matches!(s, ordo_syntax::Step::Reduce { .. }));
                    (OpKind::Read, None, fields, agg, cost, sync, refinable)
                }
            };
        specs.push(OpSpec {
            name: name.clone(),
            version_hash: op_version(f, name, op),
            kind,
            params,
            output_fields,
            // Liveness is not a property an op declares (DL §5, Live
            // queries): "named ⇒ callable ⇒ sourceable ⇒ subscribable;
            // there is no definition flag." So the only question a spec
            // can answer is whether the op is a *query* at all — a `set`
            // is an effect, not a result to hold a subscription over, and
            // the router has already pinned `GET` (and therefore `?live`)
            // to reads. Everything else about liveness is a call-time
            // choice the caller makes with `?live=1`.
            //
            // What this flag does NOT decide is whether the subscription
            // can actually see other nodes' writes: that is the bus's
            // job. A deployment wiring `MemoryBus` makes every read here
            // live-capable *and* blind to its siblings — which is why
            // [`crate::invalidation::PgInvalidationBus`] exists and why
            // it, not this line, is the honest gate on `?live`.
            live_capable: matches!(kind, OpKind::Read),
            refinable,
            aggregate_only,
            mode_label,
            cost,
            sync,
            // **Never for a compiled op.** `bootstrap.md` §2 item 44 makes
            // the header mandatory on exactly one op, and the reason is that
            // its effect lands outside the database it writes to — a KMS key
            // and a whole database. A compiled op's effect is rows in one
            // transaction, where the row's own key, a `rev` CAS or a
            // `converge` mode already bounds what a retry can do. Demanding
            // the header of every authored write would be a breaking change
            // to every client for a risk those mechanisms already cover.
            requires_idempotency_key: false,
            // `op … public` (`design-public-path.md` §1, §5). A public write
            // is a create-only write the compiler checked
            // (`ordo_plan::public`). A public delete is never one: the
            // compiler refuses `del`, and the kind is checked again here
            // because the door trusts this field.
            public: match (op.flags.public, kind) {
                (true, OpKind::Read) => ordo_api::PublicMode::Read,
                (true, OpKind::Write) => match &op.body {
                    OpBody::Write(stmts)
                        if ordo_plan::public::has_keyed_create(&f.schema, stmts) =>
                    {
                        ordo_api::PublicMode::WriteKeyed
                    }
                    _ => ordo_api::PublicMode::Write,
                },
                _ => ordo_api::PublicMode::None,
            },
        });
    }
    specs
}

/// Whether a device can keep the op in sync, named for the door.
///
/// `ordo_plan::artifact::syncability` decides; `ordo_api` re-declares its
/// refusals because it depends on no sibling crate, the same arrangement as
/// [`cost_class`]. This is the only crossing, and `tests/cost_class.rs`
/// checks both spellings over every rule.
pub fn syncability(q: &ordo_sema::ir::Query) -> Syncability {
    use ordo_plan::artifact::SyncRule;
    match ordo_plan::artifact::syncability(q) {
        Ok(s) => Syncability::Syncable {
            entity: s.entity,
            id_output: s.id_output,
        },
        Err(rule) => Syncability::NotSyncable(match rule {
            SyncRule::NoAnchor => NotSyncable::NoAnchor,
            SyncRule::Grouped => NotSyncable::Grouped,
            SyncRule::Measure => NotSyncable::Measure,
            SyncRule::Distinct => NotSyncable::Distinct,
            SyncRule::ManySideJoin => NotSyncable::ManySideJoin,
        }),
    }
}

/// The planner's cost class, named for the door.
///
/// The four classes are `ordo_plan::cost::CostClass`'s; `ordo_api` re-declares
/// them because it depends on no sibling crate. This function is the ONLY
/// crossing, and `tests/cost_class.rs` walks both inventories and both joins
/// over the whole cross product so the two spellings cannot drift into two
/// cost models.
pub fn cost_class(class: ordo_plan::cost::CostClass) -> ordo_api::CostClass {
    match class {
        ordo_plan::cost::CostClass::Sql => ordo_api::CostClass::Sql,
        ordo_plan::cost::CostClass::BlindIndex => ordo_api::CostClass::BlindIndex,
        ordo_plan::cost::CostClass::BoundedHost => ordo_api::CostClass::BoundedHost,
        ordo_plan::cost::CostClass::UnboundedHost => ordo_api::CostClass::UnboundedHost,
    }
}

/// The op's registered cost facts, for the door's per-request gate
/// (`api-engine-design.md` §7.1). Every one of them is read off the bound
/// plan by `ordo_plan::cost` — the same functions the registration gates
/// use, so the class the catalog prints, the class the venue gate enforced,
/// and the class the door joins against are one class.
fn registered_cost(f: &Fixture, q: &ordo_sema::ir::Query, unbounded_ack: bool) -> OpCost {
    OpCost {
        class: cost_class(ordo_plan::cost::classify_with(q, Some(&f.schema))),
        unbounded_ack,
        refine_candidate_bounded: matches!(
            ordo_plan::cost::refine_candidate_bound(q, &f.schema),
            ordo_plan::cost::RowBound::AtMost(_)
        ),
    }
}

/// The op's output manifest, with the per-field facts the sealed-refinement
/// rules need (`std-library.md` §2.1 rules 1–2 and rule 4).
///
/// **Sealed is not a guess**: it is membership in the query's `host_open`
/// obligation list — the compiler saying "this column leaves the database as
/// ciphertext and is opened here" — which is the same evidence the artifact's
/// `outputs[].sealed` carries. `host_open` also names the entity and field
/// the envelope is bound to, and that is what makes the declaration
/// readable: `refinable` is the author's `sealed indexed refinable` opt-in,
/// and `indexed` is whether an equality on it can ride the blind index at
/// all.
///
/// Before this, every output field was `FieldSpec::new` — open and refinable
/// — which made `refine::validate`'s sealed rules unenforceable against any
/// spec this crate produced, and left a landmine under the op-level tier: the
/// day reads became `basic` (2026-10-05, every program read, no op-level
/// word), every sealed field in a manifest would have been refinable with
/// the full operator set.
fn manifest_fields(f: &Fixture, q: &ordo_sema::ir::Query) -> Vec<FieldSpec> {
    let candidate = ordo_plan::cost::refine_candidate_bound(q, &f.schema);
    q.outputs
        .iter()
        .map(|o| {
            let ty = refine_call::field_ty(o.expr.ty());
            // Every output takes a condition, the attached anchor id
            // included (C4e, 2026-09-17: "`$id` is the attached name, and it
            // is refinable"). The op's body cannot name `$id`, so the
            // executor applies its conditions to the bound query rather than
            // to the call (`ordo_plan::artifact::filter_row_key`).
            let filter = true;
            let Some((_, entity, field)) = q.host_open.iter().find(|(name, _, _)| *name == o.name)
            else {
                // A `json` output takes no condition (28b); the door's refusal
                // names why, and the flag says so before anyone asks.
                let refinable = ty != ordo_api::FieldTy::Json;
                let mut open = FieldSpec::new(o.name.clone()).with_ty(ty);
                open.filter = filter;
                open.refinable = refinable;
                return open;
            };
            let declared = f.schema.entity(entity).and_then(|e| e.field(field));
            let indexed = declared.is_some_and(|d| d.indexed);
            // A sealed field is refinable only where its declaration says
            // so. An unknown field (a projection the schema cannot name)
            // gets the sealed default, which is "no".
            let refinable = declared.is_some_and(|d| d.refinable);
            FieldSpec {
                name: o.name.clone(),
                sealed: true,
                refinable,
                // Equality is the only operator a sealed condition may use
                // (§2.1 rule 2, enforced at the door), so this is the class
                // of the condition the door will actually let through.
                refine_class: cost_class(ordo_plan::cost::refine_field_class(
                    true, indexed, true, candidate,
                )),
                ty,
                filter,
                output: true,
            }
        })
        .collect()
}

/// `std.E.list` when `name` is `std.E.count` and the list exists: the op
/// whose manifest a count's conditions validate against (`std-library.md`
/// §2.1).
pub(crate) fn count_pair(f: &Fixture, name: &str) -> Option<String> {
    let entity = name.strip_prefix("std.")?.strip_suffix(".count")?;
    let list = format!("std.{entity}.list");
    f.registry.get(&list).map(|_| list)
}

/// `std.E.count`'s door manifest, with §2.1's pairing built (ruled
/// 2026-10-05, `refinement-default` ask 5: this round).
///
/// - Its own `n` stays the output a `pick` names, and no condition names
///   it.
/// - `std.E.list`'s fields are what a condition names. Their sealed flags
///   are the list's; their **cost** is the count's, because the condition
///   applies before the count's reduce, over everything the reduce
///   examines, which is the count's candidate set and not a page.
///
/// The spec stays `aggregate_only`, so rule 3 refuses any sealed condition
/// before cost is priced: if you may not read the rows, you may not
/// measure them.
fn count_fields(
    f: &Fixture,
    count: &ordo_sema::ir::Query,
    list: &str,
    own: Vec<FieldSpec>,
) -> Vec<FieldSpec> {
    let Ok(list_q) =
        ordo_sema::bind_op_generic(&f.schema, &f.registry, list, ordo_sema::Clock::CORPUS)
    else {
        return own;
    };
    let candidate = ordo_plan::cost::refine_candidate_bound(count, &f.schema);
    let mut fields: Vec<FieldSpec> = own
        .into_iter()
        .map(|mut fld| {
            fld.filter = false;
            fld
        })
        .collect();
    for mut fld in manifest_fields(f, &list_q) {
        if fld.sealed {
            let indexed = list_q
                .host_open
                .iter()
                .find(|(n, _, _)| *n == fld.name)
                .and_then(|(_, e, d)| f.schema.entity(e).and_then(|x| x.field(d)))
                .is_some_and(|d| d.indexed);
            fld.refine_class = cost_class(ordo_plan::cost::refine_field_class(
                true, indexed, true, candidate,
            ));
        }
        fld.output = false;
        fields.push(fld);
    }
    fields
}

/// An op's **contract** version: the digest of its own plan artifact.
///
/// One notion of version for the wire header, the page cursor and the
/// catalog manifest, because the manifest is now a projection of the same
/// artifact — two derivations would print two numbers for one op, and a
/// caller comparing the header against the catalog would see drift that
/// was not there. It also moves for the right reason: the artifact's digest
/// changes when the contract changes, where a digest of the declaration's
/// debug form moved every downstream pin when an op moved down a file.
///
/// The fallback is that older digest, for an op whose artifact cannot be
/// produced at all (a planner error). It is stable and it is a version; it
/// is simply not the contract's.
fn op_version(f: &Fixture, name: &str, op: &impl std::fmt::Debug) -> String {
    ordo_plan::artifact::op_artifact(&f.schema, &f.registry, name)
        .ok()
        .and_then(|a| a["version"].as_str().map(String::from))
        .unwrap_or_else(|| hex(&Sha256::digest(format!("{op:?}").as_bytes())[..8]))
}

/// A parameter's door-enforced bounds, from its declaration.
///
/// DL §2 Parameters: *"Bounds are enforced at the door, before planning."*
/// This is the one place the declared bound becomes an enforceable one —
/// it was `None` here, so `ParamSpec::bounds` had no producer at all and
/// every `$n: int <= 5` in the docs was a promise the compiler could not
/// make. The canonical strings are the ones `ordo_api::bind` compares
/// against: exact decimal digits for `int`/`money`, `YYYY-MM-DD` for
/// `date`.
///
/// # The implicit floor on a take count
///
/// A parameter used as `take $n` also gets `min = "0"`, and that is a
/// ruling rather than a transcription, so it is stated here. The language
/// has no `>=` bound to write it with (X-077 proposes one; it is not
/// settled), and `take -1` is already unspellable — the count is lexed as
/// an integer literal and `-` is a separate token — so a negative literal
/// count does not parse. A parameter count must not be able to say what a
/// literal count cannot: without the floor, `take $n` with `$n = -1`
/// reaches Postgres as `LIMIT -1`, which is an error, and `$n = -1` under
/// `take[keys]` is a silently empty page. The floor is derived from the
/// construct, not from the declaration, so it costs the author nothing and
/// cannot be forgotten.
///
/// The derivation itself is now [`ordo_plan::artifact::take_count_param`]
/// rather than a walk of this crate's own, and moving it closed a hole: the
/// floor lived only here, so the artifact and the interface descriptor
/// carried a `max` and said nothing about a bound the door was already
/// enforcing. One producer, two readers — the rule the field-scope manifest
/// follows, applied to the other half of a bound.
fn param_bounds(op: &ordo_syntax::OpDecl, p: &ordo_syntax::ParamDecl) -> Option<ordo_api::Bounds> {
    let max = p.max.as_ref().map(|e| match e {
        ordo_syntax::Expr::IntLit { value, .. } => value.to_string(),
        ordo_syntax::Expr::DecimalLit { digits, .. } => digits.clone(),
        ordo_syntax::Expr::DateLit { y, m, d, .. } => format!("{y:04}-{m:02}-{d:02}"),
        // `param_bound` in the parser admits no other literal shape.
        other => unreachable!("a parameter bound is an int, decimal or date literal: {other:?}"),
    });
    let min = ordo_plan::artifact::take_count_param(op, &p.name.name).then(|| "0".to_string());
    if max.is_none() && min.is_none() {
        return None;
    }
    Some(ordo_api::Bounds { min, max })
}

/// The declared type of a parameter, as the door's inventory spells it.
///
/// # This was the most dangerous arm in the tree, and it is now a match
///
/// It read `_ => ParamTy::Text { max_len: None }`. That wildcard was not a
/// shorthand for the remaining types; it was a decision made by accident
/// for every type nobody had got to yet, and it had already been made
/// wrongly: `$t: datetime` parses, has a test in `declparse.rs`, and bound
/// at the door as **unbounded text with no ISO-8601 validation**. Nobody
/// noticed because nothing broke — which is precisely what a wildcard buys
/// and precisely why it costs.
///
/// It is also the one place a new `ScalarTy` slips through silently rather
/// than breaking the build, and it is the *door* — the place where "it
/// bound as text" becomes a caller-supplied string reaching a plan. So it
/// is exhaustive now, and `None` is a real answer rather than a fallback.
///
/// `None` means **this type is not a parameter type**:
///
/// - `decimal` has no declaration form at all (DL §2), so there is no
///   parameter to type.
/// - `blob` is bytes, and bytes are not a JSON wire value. The compiler
///   refuses `$f: blob` with class `blob_parameter`, so reaching `None`
///   here would mean that refusal regressed — which the caller treats as
///   the invariant violation it is rather than defaulting to text.
fn param_ty(p: &ordo_syntax::ParamDecl) -> Option<ParamTy> {
    Some(match &p.ty {
        // `$payee: text(64)` — the declared length bound rides to the
        // door, which counts characters and answers `bound_violation`
        // (`api-engine-design.md` §5). Before this the slot existed and
        // nothing authored ever filled it.
        ScalarTy::Text => ParamTy::Text {
            max_len: p.text_max,
        },
        ScalarTy::Int => ParamTy::Int,
        ScalarTy::Money(places) => ParamTy::Money { places: *places },
        ScalarTy::Decimal(places) => ParamTy::Decimal { places: *places },
        ScalarTy::Date => ParamTy::Date,
        ScalarTy::Datetime => ParamTy::Datetime,
        ScalarTy::Bool => ParamTy::Bool,
        ScalarTy::Id => ParamTy::Id,
        ScalarTy::Duration(_) => ParamTy::Duration,
        ScalarTy::Uri => ParamTy::Uri,
        ScalarTy::Json => ParamTy::Json,
        ScalarTy::Blob => return None,
    })
}

/// A JSON value as the source string literal that carries it (28b).
///
/// `serde_json`'s compact form escapes every control character as `\n`,
/// `\u0001` and so on, so the text holds no raw control character; what is
/// left to escape for the lexer's closed set is `"` and `\`. The lexer
/// undoes exactly that, so the binder sees the compact text unchanged.
pub fn json_source_literal(v: &serde_json::Value) -> String {
    let text = serde_json::to_string(v).unwrap_or_else(|_| "null".into());
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A caller's text as the string literal a call carries it in: the
/// language's closed escape set (`\"`, `\\`, `\n`, `\t`), so the value the
/// lexer reads back is the value the door bound. Every other character
/// rides raw, which the lexer takes; a NUL was refused at the door.
pub fn text_source_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The door, answered from platform membership: the conformance token form
/// is `member:<id>`, and the principal must exist in the space's `members`
/// entity. A missing member, an unknown space, and a malformed token are
/// one `Deny` — never distinct answers (enumeration).
pub struct MemberAccess {
    pub fixture: Arc<Fixture>,
    pub pg: Pg,
}

impl MemberAccess {
    /// This checker's fixture bound to one space.
    ///
    /// Cut per call and not cached, unlike [`spaces::Spaces`]'s views,
    /// because the two are paying for different things: a view is on the
    /// request's execution path and is cut once per space per version,
    /// while this is the conformance tier's access checker and a deployment
    /// runs [`IdentityAccess`] instead. Stated rather than left to be
    /// discovered: a real door doing a schema clone per check would be a
    /// cost worth caching, and if this one ever becomes that, this is the
    /// line to change.
    fn scoped(&self, space: &str) -> Fixture {
        self.fixture.in_space(space)
    }
}

impl AccessChecker for MemberAccess {
    fn check(&self, token: &str, space: &str, _op: &str) -> AccessDecision {
        let Some(id) = token.strip_prefix("member:") else {
            return AccessDecision::Deny;
        };
        if !id.starts_with("mem_") || id.contains('\'') {
            return AccessDecision::Deny;
        }
        // **The space the request named**, not the one this fixture was
        // built in. This used to compare the space against a constant and
        // deny everything else, which was a single-space host refusing
        // honestly; with the host serving many, the same refusal would
        // deny every tenant but one. The membership query is the whole
        // answer either way, and it is per space because `in_space` is
        // what confines its statement. An unprovisioned space simply has
        // no `members` row to find, which is the same `Deny` a
        // non-member gets and discloses nothing about which spaces exist.
        let scoped = self.scoped(space);
        let visible = scoped
            .run_case_params(
                &self.pg,
                "members(id == $m)[displayName]",
                &[("m", &format!("\"{id}\""))],
            )
            .map(|rows| rows.as_array().map(|a| !a.is_empty()).unwrap_or(false))
            .unwrap_or(false);
        if visible {
            AccessDecision::allow(id)
        } else {
            AccessDecision::Deny
        }
    }
}

/// The executor: reads bind-and-plan through `ordo-sema`/`ordo-plan` with
/// the caller in the guard slot; writes run the Stage-5 protocol and land
/// in the command chain of the space the door bound.
///
/// **The program is read per call, not held.** It used to own an
/// `Arc<Fixture>` and an `ordo_exec::Executor` built over that fixture's
/// schema and registry, which meant a hot swap of the door's registry left
/// this executor planning the previous program — a half-swap where each
/// half is individually valid and the pair is wrong. Both now come out of
/// [`deploy::Deployment`], which is also what the door dispatches through,
/// so there is one version and one place it changes.
pub struct FixtureExecutor {
    /// The live program, and the space binding resolved against it.
    ///
    /// One read per call gives the version (cloned, so an in-flight request
    /// finishes on the version it started with) *and* the executor, fixture
    /// and chain for the space the door bound. It used to be the deployment
    /// plus one shared `WriteLog`, which was a single-space deployment
    /// spelled out in two fields; see [`spaces`].
    pub spaces: Arc<spaces::Spaces>,
    pub pg: Pg,
    /// Rows per page. A deployment-wide bound, not a caller's choice: page
    /// size is a cost decision, and the cost gate is the platform's.
    pub page_size: i64,
}

impl FixtureExecutor {
    /// `$caller` arrives in the bound params (router-inserted from the
    /// access decision — a reserved key no op parameter can shadow).
    fn caller_of(bound: &BoundParams) -> Result<String, ExecError> {
        bound
            .0
            .get("$caller")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| ExecError::internal("no principal reached the executor"))
    }

    /// The agent acting for `$caller` under a delegation, when the door
    /// admitted one ([`ordo_api::AGENT_PARAM`]). Read for the command
    /// group's actor only; it never reaches a guard.
    fn agent_of(bound: &BoundParams) -> Option<String> {
        bound
            .0
            .get(ordo_api::AGENT_PARAM)
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn call_source(op: &OpSpec, bound: &BoundParams) -> Result<String, ExecError> {
        let mut args = Vec::new();
        for p in &op.params {
            let Some(v) = bound.0.get(&p.name) else {
                continue;
            };
            let lit = match (&p.ty, v) {
                // Money arrives as a decimal string in major units and is
                // rendered as the literal the source would write; the
                // binder converts it at the parameter's places (27ak).
                (ParamTy::Int | ParamTy::Money { .. } | ParamTy::Decimal { .. }, v) => {
                    v.to_string().trim_matches('"').to_string()
                }
                (ParamTy::Bool, serde_json::Value::Bool(b)) => b.to_string(),
                // A document rides the call as its compact JSON text in a
                // string literal, escaped for the language's closed set
                // (28b). Compact JSON escapes every control character
                // itself, so only `"` and `\` need escaping here; the
                // quote-stripping arm below would change the document.
                (ParamTy::Json, v) => json_source_literal(v),
                (ParamTy::Date, serde_json::Value::String(s)) => s.clone(),
                // Text, an id, a datetime and an address ride in a string
                // literal escaped for the language's closed set, as the
                // import door spells them. This arm used to strip every
                // `"` from the value and leave `\` raw, so `He said "hi"`
                // was stored as `He said hi` and a value holding a
                // backslash failed to parse (audit 2026-10-08).
                (_, serde_json::Value::String(s)) => text_source_literal(s),
                (_, other) => other.to_string(),
            };
            args.push(format!("${}: {}", p.name, lit));
        }
        Ok(if args.is_empty() {
            op.name.clone()
        } else {
            format!("{}({})", op.name, args.join(", "))
        })
    }
}

/// The tiers below the transport answer with a class-prefixed string
/// (`"tree_cycle: …"`); the wire inventory is a closed enum. This is the
/// one place the two meet.
///
/// A prefix that names no class — `parse:`, `bind:`, `plan:`, and every
/// unprefixed message — is `internal` (500) and should be: a *registered*
/// op that fails to parse, bind, or plan at request time is a server bug,
/// not something a caller can act on. But a prefix that names a class the
/// inventory has and this function does not falls through the same hole,
/// and *that* turns a rule the platform built into a 500 the caller cannot
/// distinguish from a crash. That was 9g: seventeen typed refusals, seven
/// of them classified.
///
/// # The hazard this seam still carries
///
/// It is a **string** seam. `ordo-conformance`'s write protocol returns
/// `Result<_, String>` and encodes its class in a prefix by convention;
/// nothing in the type system requires a new refusal to carry one, or to
/// spell it the way the inventory spells it. A typo (`tree_cyle:`) or a
/// missing prefix is a silent demotion to 500, and no compiler will say so.
///
/// The seam is not closed here because closing it means changing the write
/// protocol's error type from `String` to a typed refusal across ~2,300
/// lines of `write.rs` — the right fix, and a change that wants the write
/// path to itself, not a drive-by from the transport crate. Two things
/// stand in for it in the meantime:
///
/// 1. this function no longer enumerates classes by hand — it asks the
///    inventory ([`ordo_api::ErrorCode::from_class`]), so adding a code is
///    the only step, and
/// 2. `tests/http.rs::every_raised_class_prefix_is_a_wire_class` reads the
///    write path's and the executor's own source and fails if any prefix
///    they raise is not a class this function will recognise.
///
/// (2) is a static tripwire, not a type. It catches the misspelling and the
/// forgotten inventory entry; it does not catch a refusal that never had a
/// prefix at all.
pub fn classify(e: String) -> ExecError {
    let Some((class, rest)) = e.split_once(':') else {
        return ExecError::internal(e);
    };
    // A class the executor may not say (`access_denied` above all) is a bug
    // in whatever said it, so it is `internal` too — never a 403 the door
    // did not issue.
    match ordo_api::ErrorCode::from_class(class).filter(|c| c.executor_raisable()) {
        Some(code) => ExecError::new(code, rest.trim()),
        None => ExecError::internal(e),
    }
}

/// The executor's failure, in the executor's vocabulary. The database's own
/// class is read from its type first (`ExecFail::is_busy`, owner ruling
/// 2026-10-23: one typed error per service); what remains is the write
/// protocol's `class: message` text, read by [`classify`].
/// A failure that knows its [`ordo_fault::FaultClass`], so an op that meets
/// it answers by type: a busy one is `server_busy`, anything else the op's
/// own class ([`ExecError::of_fault`]). A sentence knows no class, so it is
/// broken. The door never reads the words to find a busy pool (fix order
/// step 6, `docs/audit-service-interfaces.md`).
pub trait Faulty: std::fmt::Display {
    fn fault_class(&self) -> ordo_fault::FaultClass {
        ordo_fault::FaultClass::Broken
    }
}

impl Faulty for ordo_exec::PoolError {
    fn fault_class(&self) -> ordo_fault::FaultClass {
        self.class()
    }
}

impl Faulty for ordo_identity::StoreFault {
    fn fault_class(&self) -> ordo_fault::FaultClass {
        self.class()
    }
}

impl Faulty for spaces::SpaceRefusal {
    fn fault_class(&self) -> ordo_fault::FaultClass {
        self.class()
    }
}

impl Faulty for ordo_exec::ExecFail {
    fn fault_class(&self) -> ordo_fault::FaultClass {
        self.pool_error()
            .map_or(ordo_fault::FaultClass::Broken, ordo_exec::PoolError::class)
    }
}

impl Faulty for String {}
impl Faulty for &str {}

/// A store that did not answer an op: `server_busy` when it was busy,
/// `space_unavailable` with its sentence otherwise.
pub(crate) fn op_store_unavailable(e: impl Faulty) -> ExecError {
    ExecError::of_fault(e.fault_class(), ErrorCode::SpaceUnavailable, e.to_string())
}

pub fn exec_failure(e: ordo_exec::ExecFail) -> ExecError {
    if e.is_busy() {
        return ExecError::server_busy();
    }
    classify(e.to_string())
}

/// A bind failure of a read that carried a `$refine` with conditions is the
/// caller's refusal, not a server fault (25av, 2026-10-09).
///
/// The binder's message arrives as `bind: [code] text at span` (or
/// `bind: text` from the `$id` filter). The door has already checked the
/// refinement against the manifest, so a failure here is a condition the
/// binder cannot place. It answers its class and names the field:
/// `sealed_unsupported` stays that class, every other bind code is
/// `invalid_refinement`. A bind failure with no condition to blame, or any
/// other message, is `None` and falls to [`classify`] (500).
pub fn refine_bind_refusal(text: &str, refine: Option<&ordo_api::Refinement>) -> Option<ExecError> {
    let r = refine.filter(|r| !r.where_.is_empty())?;
    let rest = text.strip_prefix("bind: ")?;
    let (code, body) = match rest.strip_prefix('[').and_then(|t| t.split_once("] ")) {
        Some((code, body)) => (Some(code), body),
        None => (None, rest),
    };
    let body = body.rsplit_once(" at ").map_or(body, |(b, _)| b);
    let named: Vec<String> = r
        .where_
        .iter()
        .map(|c| c.field.as_str())
        .filter(|f| body.contains(&format!("`{f}`")))
        .map(|f| format!("`{f}`"))
        .collect();
    let fields = if named.is_empty() {
        let mut all: Vec<String> = r.where_.iter().map(|c| format!("`{}`", c.field)).collect();
        all.dedup();
        all.join(", ")
    } else {
        named.join(", ")
    };
    let class = match code {
        Some("sealed_unsupported") => ordo_api::ErrorCode::SealedUnsupported,
        _ => ordo_api::ErrorCode::InvalidRefinement,
    };
    Some(ExecError::new(
        class,
        format!(
            "this refinement cannot be applied to this op (condition on {fields}): {body}. \
             Drop or change the condition."
        ),
    ))
}

impl OpExecutor for FixtureExecutor {
    fn execute<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: BoundParams,
        refine: Option<ordo_api::Refinement>,
        page: Option<ordo_api::PageCursor>,
        lease: Option<ReplayLease>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecError>> {
        self.execute_sized(op, space, bound, refine, page, lease, None)
    }

    fn execute_sized<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: BoundParams,
        refine: Option<ordo_api::Refinement>,
        _page: Option<ordo_api::PageCursor>,
        lease: Option<ReplayLease>,
        page_rows: Option<usize>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecError>> {
        Box::pin(async move {
            let caller = Self::caller_of(&bound)?;
            let agent = Self::agent_of(&bound);
            let call = Self::call_source(op, &bound)?;
            // One read of the live program AND of the space binding, held
            // for the whole call: a swap landing mid-request must not move
            // the schema out from under a write that has already planned
            // against it.
            //
            // This used to assert `space == SPACE` and answer `internal`
            // otherwise, which was the honest refusal of a host that held
            // one executor. It holds one per space now, so the assertion
            // has nothing left to protect: the space is not compared
            // against a constant, it selects the executor, and the
            // executor's own `space` field is what every statement below is
            // confined to. `Gate::approved_space` has already checked that
            // this string is the space the door *authorized*, which is the
            // check the assertion was standing next to and is the one that
            // matters.
            let bound_space = self.spaces.resolve(space).await.map_err(space_refusal)?;
            let view = &bound_space.view;
            match op.kind {
                OpKind::Read => {
                    // The cursor's keys are text on the way out and back;
                    // JSON strings are their wire form, and `null` is a
                    // NULL key (nulls sort last, so they page like any
                    // other value).
                    let after: Option<Vec<Option<String>>> = _page.map(|c| {
                        c.keys
                            .iter()
                            .map(|v| match v {
                                serde_json::Value::Null => None,
                                serde_json::Value::String(s) => Some(s.clone()),
                                other => Some(other.to_string()),
                            })
                            .collect()
                    });
                    // The call is built as a pipeline, with the refinement's
                    // conditions and pick compiled into it (ruled
                    // 2026-10-05, `refinement-default` ask 1). It used to
                    // take `_refine` and drop it, which served the op's
                    // unfiltered rows to a caller who had asked for fewer.
                    let count_of = refine
                        .as_ref()
                        .filter(|r| !r.where_.is_empty())
                        .and_then(|_| count_pair(&view.fixture, &op.name));
                    let built =
                        refine_call::build(op, &bound, refine.as_ref(), count_of.as_deref())?;
                    // One correlation for this request. A page that returns
                    // sealed values takes one read release under it, and the
                    // metered provider records that release with it.
                    let correlation = ordo_secevent::correlation::mint();
                    let mut page = view
                        .exec
                        .read_page_pipeline(
                            &built.pipeline,
                            &built.row_key,
                            &call,
                            Some(&caller),
                            page_rows.map_or(self.page_size, |n| n as i64),
                            after.as_deref(),
                            &correlation.0,
                        )
                        .await
                        .map_err(|e| {
                            if e.is_busy() {
                                return ExecError::server_busy();
                            }
                            let text = e.to_string();
                            refine_bind_refusal(&text, refine.as_ref())
                                .unwrap_or_else(|| classify(text))
                        })?;
                    if let Some(keep) = &built.keep_only {
                        for row in &mut page.rows {
                            if let serde_json::Value::Object(obj) = row {
                                obj.retain(|k, _| keep.contains(k));
                            }
                        }
                    }
                    Ok(ExecOutcome::Ran(ExecResult {
                        position: page.position,
                        rows: page.rows,
                        next_cursor: page.next.map(|keys| ordo_api::PageCursor {
                            keys: keys
                                .into_iter()
                                .map(|k| match k {
                                    None => serde_json::Value::Null,
                                    Some(s) => serde_json::Value::String(s),
                                })
                                .collect(),
                        }),
                    }))
                }
                OpKind::Write | OpKind::Delete => {
                    // Chained: the group joins this space's durable chain in
                    // the write's own transaction. Before this the host
                    // built groups in memory only, so a restart lost every
                    // space's history (checklist 11-28).
                    // Under a delegation `caller` is the delegator and
                    // `agent` the token's holder: the guards see the first,
                    // and the chain names both.
                    let summary = view
                        .exec
                        .write_chained_as(
                            view.fixture.clone(),
                            bound_space.log.clone(),
                            call,
                            caller,
                            agent,
                            lease.map(|l| (l.key, l.fingerprint)),
                        )
                        .await
                        .map_err(exec_failure)?;
                    Ok(match summary {
                        Some(v) => ExecOutcome::Ran(ExecResult {
                            position: None,
                            rows: v.as_array().cloned().unwrap_or_default(),
                            next_cursor: None,
                        }),
                        None => ExecOutcome::AlreadyApplied,
                    })
                }
            }
        })
    }
}

// ── Stage 8: ops truth — the manifest a catalog and a consent page render ─

/// Everything the platform knows about an op, as data — **a projection of
/// [`ordo_plan::artifact::op_artifact`]**, not a second assembly of the
/// same facts.
///
/// This closes the reduction 9m left owed. The manifest and the plan
/// artifact answered the same questions (what does this op touch, what
/// guards it, what does it cost, what mode does it write in) from two
/// walks over the same compiler output, and two walks are two answers
/// waiting to differ — which they already did: the artifact grew the
/// field-scope manifest and the per-statement mode list, and the manifest
/// would not have. There is now exactly one producer, at the crate that
/// produces every field of it, and this function selects and renders.
///
/// Three deliberate differences from the artifact, all of them subtraction
/// or presentation:
///
/// - **The planned SQL is withheld.** The catalog is published to every
///   caller who may invoke the op; the statement is the deployment's
///   internals, and no consent question is answered by it. The artifact —
///   a deploy-time document read by operators — keeps it.
/// - **`cost.rows` is prose** (`<= 3`, `unbounded (pageable)`), because the
///   catalog's two renderings are read by a human and by a model. The
///   artifact carries the number, so a reworded string is never a deploy
///   diff.
/// - **`flags` is flattened** to the two disclosures a consent page states
///   (`unguarded`, `unbounded`) with `bypassed_guards` beside them (R-G8:
///   the flag never travels alone).
///
/// The guard set carries **provenance** — own, inherited (naming the
/// entity it came from), or acl-derived — because an inlined guard that
/// does not disclose is half-built (engine-plan §Track P): the consent
/// page renders exactly this. And `scope` states WHICH FIELDS (9l), which
/// is the question an entity list could not answer.
pub fn manifest(f: &Fixture, name: &str) -> Option<serde_json::Value> {
    let art = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, name).ok()?;
    Some(ordo_plan::artifact::catalog_view(&art))
}

/// Every op's manifest, op-name sorted.
///
/// The catalog route renders this at boot and filters it per asker: it is
/// one bind and one plan per op, which is a deploy-time cost and not a
/// request-path one.
pub fn manifests(f: &Fixture) -> Vec<(String, serde_json::Value)> {
    let mut names: Vec<&String> = f.registry.names().collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|n| Some((n.clone(), manifest(f, n)?)))
        .collect()
}

/// The projection itself: artifact in, catalog manifest out.
///
/// **Moved down to the producer** ([`ordo_plan::artifact::catalog_view`]).
/// Every field it selects is compiler output, and this crate is the wiring
/// crate — `artifact.rs`'s own layering argument, applied to the rendering
/// as well as to the assembly. It moved because the interface descriptor
/// (`ordo_plan::interface`) is a second projection of the same manifest and
/// is built by a CLI that must not depend on a `Fixture`, a pool or an HTTP
/// stack to describe a program that has never met a database.
///
/// What stays here is what this crate actually owns: **who may see which
/// op** ([`catalog_for`]).
pub(crate) fn collect_entities(
    f: &Fixture,
    name: &str,
    out: &mut Vec<String>,
    seen: &mut Vec<String>,
) {
    if seen.contains(&name.to_string()) {
        return;
    }
    seen.push(name.to_string());
    let Some(op) = f.registry.get(name) else {
        return;
    };
    let mut srcs: Vec<String> = Vec::new();
    match &op.body {
        OpBody::Read(pl) => pipeline_sources(pl, &mut srcs),
        OpBody::Write(stmts) => {
            for s in stmts {
                srcs.push(match s {
                    WriteStmt::Set { entity, .. } | WriteStmt::Del { entity, .. } => {
                        entity.name.clone()
                    }
                });
            }
        }
    }
    for s in srcs {
        if f.schema.entity(&s).is_some() {
            out.push(s);
        } else {
            collect_entities(f, &s, out, seen);
        }
    }
}

fn pipeline_sources(pl: &ordo_syntax::Pipeline, out: &mut Vec<String>) {
    for l in &pl.lets {
        pipeline_sources(&l.pipeline, out);
    }
    for s in &pl.sources {
        out.push(s.name.name.clone());
    }
    for st in &pl.steps {
        match st {
            ordo_syntax::Step::Match {
                kind: ordo_syntax::decl::MatchKind::Any(s) | ordo_syntax::decl::MatchKind::None(s),
                ..
            }
            | ordo_syntax::Step::Append { source: s, .. } => out.push(s.name.name.clone()),
            _ => {}
        }
    }
}

/// The catalog **scoped to the asker** (F6, settled): a token sees exactly
/// the ops it may call, because an agent's catalog IS its consent page in
/// machine form.
///
/// Scoping is not decoration. An unscoped catalog is an enumeration of the
/// deployment's whole surface handed to anyone who asks — op names, the
/// entities they touch, the guards that protect them — which is the same
/// disclosure the door refuses one request at a time. So the catalog runs
/// the same check the door runs, per op, and omits rather than marks: a
/// listed-but-forbidden op still tells the asker it exists.
pub fn catalog_for(
    f: &Fixture,
    access: &dyn AccessChecker,
    token: &str,
    space: &str,
) -> serde_json::Value {
    serde_json::Value::Array(
        manifests(f)
            .into_iter()
            .filter(|(n, _)| matches!(access.check(token, space, n), AccessDecision::Allow { .. }))
            .map(|(_, m)| m)
            .collect(),
    )
}

/// The **Interface Descriptor scoped to the asker** — B1's document served
/// beside the catalog, per `ui-gen.md` §1's *descriptors are themselves
/// catalog-scoped*.
///
/// It is [`catalog_for`] with a second projection on the end, and the order
/// is the whole of the guarantee: the manifests are filtered by the same
/// check the door runs, and only then does
/// [`ordo_plan::interface::interface_document`] describe them. So the
/// descriptor a caller receives contains **exactly** the ops their catalog
/// shows — not by a rule stated twice, but by there being one filter.
///
/// Three consequences worth naming, because each is a leak that did not
/// happen:
///
/// - **Entities follow ops.** The entity half describes only entities a
///   visible op reaches, so a viewer with three read ops does not receive
///   the shape of every table in the deployment.
/// - **Action discovery follows ops.** The typed-id index is built from the
///   filtered list, so a viewer cannot learn that a manager-only
///   `archiveProject` exists by noticing that `proj_` ids have actions.
/// - **Refusal metadata answers about the op it is on.** It is derived from
///   that op's own declaration and never from a lookup keyed on a name the
///   asker may not see — the rule `std.space.callersOf` follows when it is
///   asked about an op outside the asker's catalog.
///
/// The digest differs per asker, and that is correct: two callers holding
/// different surfaces hold different contracts, and a shared digest would
/// tell each of them their document was the whole one.
pub fn interface_for(
    f: &Fixture,
    access: &dyn AccessChecker,
    token: &str,
    space: &str,
) -> serde_json::Value {
    let ops: Vec<serde_json::Value> = manifests(f)
        .into_iter()
        .filter(|(n, _)| matches!(access.check(token, space, n), AccessDecision::Allow { .. }))
        .map(|(_, m)| m)
        .collect();
    let limits = ordo_api::RefineLimits::from_env();
    ordo_plan::interface::interface_document_with(
        &serde_json::json!(f.schema.domain),
        &ordo_plan::artifact::entity_artifacts(&f.schema),
        &ops,
        // The limits the door enforces, so a client's check before the wire
        // and the door's refusal are one number.
        ordo_plan::interface::RefineLimits {
            conditions: limits.max_conditions,
            in_values: limits.max_in_values,
        },
    )
}

/// The catalog as an **LLM-ready document**: compact prose and signatures,
/// the same content as the JSON rendering. Two renderings, one source —
/// so an agent's context can carry the callable surface without a second
/// artifact to keep in sync.
pub fn catalog_prose(catalog: &serde_json::Value) -> String {
    let mut out = String::from(
        "# Callable operations\n\nEvery op below is one you may call. Each names the \
         exact FIELDS it reads and writes, what it costs, the guards that decide \
         which rows you see, and any key release it needs. A line marked `read by a \
         guard, not by the op` is data the platform examines on your behalf to \
         decide what you may see — you never receive it, and it is still \
         disclosure.\n",
    );
    for m in catalog.as_array().unwrap_or(&Vec::new()) {
        let name = m["op"].as_str().unwrap_or("?");
        let kind = m["kind"].as_str().unwrap_or("read");
        let params: Vec<String> = m["params"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|p| {
                        format!(
                            "${}{}",
                            p["name"].as_str().unwrap_or("?"),
                            if p["required"].as_bool().unwrap_or(false) {
                                ""
                            } else {
                                "?"
                            }
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push_str(&format!(
            "\n## {name}({})\n{kind}",
            params.join(", "),
            kind = kind
        ));
        if let Some(mode) = m["mode"].as_str() {
            out.push_str(&format!(" · {mode}"));
        }
        if let Some(class) = m["cost"]["class"].as_str() {
            out.push_str(&format!(
                " · cost {class} · rows {}",
                m["cost"]["rows"].as_str().unwrap_or("?")
            ));
        }
        out.push('\n');
        if let Some(e) = m["error"]["code"].as_str() {
            out.push_str(&format!(
                "DOES NOT COMPILE ({e}): {}\n",
                m["error"]["message"].as_str().unwrap_or("")
            ));
        }
        // §6 rule 1: the union is on the heading line above; a
        // multi-statement op also gets its statements, in order, because a
        // union alone cannot say which entity got which treatment.
        let stmts = m["statements"].as_array().cloned().unwrap_or_default();
        if stmts.len() > 1 {
            out.push_str(&format!(
                "statements: {}\n",
                stmts
                    .iter()
                    .map(|s| format!(
                        "{} {} — {}",
                        s["verb"].as_str().unwrap_or("?"),
                        s["entity"].as_str().unwrap_or("?"),
                        s["label"].as_str().unwrap_or("?")
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let outs: Vec<&str> = m["outputs"]
            .as_array()
            .map(|a| a.iter().filter_map(|o| o["name"].as_str()).collect())
            .unwrap_or_default();
        if !outs.is_empty() {
            out.push_str(&format!("returns: {}\n", outs.join(", ")));
        }
        // The field-scope manifest (9l). This replaces the old entity list
        // outright rather than sitting beside it: `touches: tasks` and
        // `reads: tasks.{status, title, project}` answer the same question
        // at two grains, and the coarse one is the one that let a caller
        // consent to a whole entity for an op that reads three columns.
        for s in m["scope"].as_array().unwrap_or(&Vec::new()) {
            out.push_str(&format!(
                "reaches: {}\n",
                ordo_plan::artifact::render_scope_json(s)
            ));
        }
        if let Some(purpose) = m["release"]["purpose"].as_str() {
            let keys: Vec<String> = m["release"]["keys"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|k| {
                            format!(
                                "{}.{} ({} key, {} stage)",
                                k["entity"].as_str().unwrap_or("?"),
                                k["field"].as_str().unwrap_or("?"),
                                k["key"].as_str().unwrap_or("?"),
                                k["stage"].as_str().unwrap_or("?"),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push_str(&format!("key release ({purpose}): {}\n", keys.join(", ")));
        }
        for g in m["guards"].as_array().unwrap_or(&Vec::new()) {
            out.push_str(&format!(
                "guard on {} ({}): {}\n",
                g["entity"].as_str().unwrap_or("?"),
                g["source"].as_str().unwrap_or("?"),
                g["read"].as_str().unwrap_or("?")
            ));
        }
        if m["unguarded"].as_bool().unwrap_or(false) {
            out.push_str(
                "WARNING: this op bypasses row guards — results are the same for \
                          every caller.\n",
            );
        }
    }
    out
}

/// The whole catalog, op-name sorted — what a deployment publishes.
pub fn catalog(f: &Fixture) -> serde_json::Value {
    serde_json::Value::Array(manifests(f).into_iter().map(|(_, m)| m).collect())
}

// ── real credentials: the door over verified claims ──────────────────────

/// The door, answered properly: a signed bearer token is verified into
/// [`TokenClaims`], and `AccessEngine` decides. Replaces `MemberAccess`,
/// whose `member:<id>` form asserted a principal rather than proving one.
///
/// Two properties the wire depends on:
///
/// - **One answer.** Every refusal — bad signature, expired token,
///   no membership, suspended, op not in role, capability outside its
///   allowlist or space, stale step-up — is the same `Deny`. The
///   *reasons* are rich and they go to the operator's audit path; a caller
///   who could tell them apart could enumerate memberships and op names by
///   watching which complaint changes.
/// - **The clock is an input.** `now` comes from the checker, not from the
///   token, so an expiry cannot be outrun by a caller lying about the
///   time, and a suspension takes effect on the next request rather than
///   at the next expiry.
pub struct IdentityAccess {
    /// The access state, **shared**. An `Arc` rather than an owned engine
    /// because `ordo_host::platform_ops::PlatformOps` reads the same state
    /// to answer `std.space.callersOf` (8r), and an audit surface computed
    /// from a second copy would answer about a deployment that is not the
    /// one serving. One engine, one answer, one place a membership change
    /// lands.
    pub engine: Arc<ordo_identity::AccessEngine>,
    pub key: ordo_identity::TokenKey,
    /// Wall clock, injectable so conformance can pin it.
    pub now: Box<dyn Fn() -> u64 + Send + Sync>,
    /// Where denials go with their reasons. The wire never sees these.
    ///
    /// This **replaces** the old `Mutex<Vec<String>>` audit buffer, which
    /// was a test-observation device rather than a record: it was drained by
    /// whoever asked first, it lived only in this process, and nothing could
    /// check that it had not been edited. What goes in now is the routed,
    /// collapsible input to the hash-chained security-event stream (7a,
    /// `security-logging-design.md` §6.1). [`IdentityAccess::take_audit`]
    /// still renders the same prose out of it, so an operator reading a
    /// denial sees what they saw before — the difference is that the prose
    /// is now a *rendering of* a record instead of the only trace.
    ///
    /// Nothing here does I/O. §6.1 pins that stream appends run off the
    /// request path: a denial must never delay a legitimate write, and a
    /// failed append is an operational alert rather than a failed request.
    /// A deployment drains this recorder from the job queue into a
    /// `SecurityStore`; with no drainer wired, entries accumulate in memory
    /// and `take_audit` is the drain.
    pub stream: Arc<ordo_secevent::Recorder>,
    /// The counter substrate. Every denial increments
    /// `ordo_denials_total{space,principal,principal_kind,op}` here, which
    /// is the mandatory deny-volume-per-principal-per-space alert's only
    /// input (`implementation.md` §Observability, checklist 7b).
    ///
    /// The door is the right place for it and the audit trail is not: the
    /// audit vector is drained by whoever asks and is unaggregated prose,
    /// while an alert needs a monotone count per series. They record the
    /// same events for different readers.
    pub metrics: Arc<metrics::Metrics>,
    /// Where the door asks whether a claimed delegation has been withdrawn
    /// — the one authorization question a bearer string cannot answer.
    ///
    /// `None` is a deployment that records no delegations, and it is not a
    /// hole: [`IdentityAccess::token_gate`] then refuses **every** token
    /// that claims one, because a delegation nothing can confirm is a
    /// delegation nobody can revoke. Failing closed on exactly the calls
    /// whose authority cannot be checked, and on nothing else, is what keeps
    /// DL §8's word "revocable" true — see `ordo_identity::delegation` for
    /// the argument, including why this registry sits in the request path
    /// when the grant registry deliberately does not.
    pub delegations: Option<Arc<dyn ordo_identity::delegation::DelegationRegistry>>,
    /// Where the door asks whether a session has been logged out: the
    /// second authorization question a bearer string cannot answer, and the
    /// one that runs on **every** request.
    ///
    /// # What it costs per request
    ///
    /// One `RwLock` read acquisition and one `BTreeSet` lookup over the
    /// revoked session ids: a handful of comparisons of a 32-character
    /// string, no allocation, no syscall, **no I/O**. It runs after
    /// `ordo_identity::verify` has already computed an HMAC-SHA256 over the
    /// payload, and it is not measurable beside that. Contrast the
    /// delegation registry above, which *is* a database round trip and is
    /// paid only by the calls that claim a delegation.
    ///
    /// That cost is what `ordo_host::sessions::PgSessionRevocations` is
    /// shaped around: the list lives in memory, is loaded when the store is
    /// built, is written through on logout and is re-read on a cadence. The
    /// door never queries.
    ///
    /// # `None`, and why it is not a hole
    ///
    /// A deployment with no revocation store cannot have revoked anything,
    /// because [`crate::auth::logout`] is served from the same store and
    /// answers `auth_unconfigured` without one. So `None` is "no logout
    /// exists here", which is exactly what the old behaviour was, rather
    /// than "logouts happen and are ignored". This differs from the
    /// delegation registry on purpose: a delegation can be recorded by
    /// another process and a missing registry means unconfirmable, while a
    /// revocation can only arrive through this same deployment's own route.
    ///
    /// # Only a session is checked
    ///
    /// A token with no `SessionClaim` is not a session: a capability token
    /// and a delegated token are each ended by withdrawing the grant or the
    /// consent they were minted against, which the registry above and the
    /// grant registry already answer. Nothing is skipped by this.
    pub revocations: Option<Arc<dyn ordo_identity::session::SessionRevocations>>,
    /// Where the door asks whether a **share** grant (`shr_`) is still live
    /// (`plan-sharing.md` §3.5, G5). The person who revokes a share is not the
    /// holder of the window token and gets no say in its life, so, on
    /// delegation's argument, the registry is consulted for these grants on
    /// every call that presents one and for no other grant. `None` refuses
    /// every `shr_` token: a share nothing can confirm is a share nobody can
    /// revoke. An unreachable registry refuses them too, and refuses nothing
    /// else. Ordinary capability grants keep the rule that the registry is
    /// never consulted to admit a request.
    pub shares: Option<Arc<dyn ordo_identity::grants::GrantRegistry>>,
    /// What each tenant requires of a session's second step, and what the
    /// deployment does (`plan-second-factor.md` §5.4). Set once, from the
    /// object the sign-in gate and the admission op share, so a level an owner
    /// sets is the level the next request meets. Empty, the door asks nothing
    /// of a session's second step, which is every deployment without one
    /// configured.
    pub second_step: std::sync::OnceLock<Arc<second_factors::SecondStepLevels>>,
    /// The deployment's own space, the one space a console key session
    /// reaches (`console_keys::console_may`). Unset, a console key session
    /// reaches none.
    pub console_home: std::sync::OnceLock<String>,
    /// The identity service's keys (`plan-identity-service.md` 4.2): what a
    /// deployment token, 3 segments, is verified with. Unset, the door
    /// accepts no deployment token, and records each one it refuses.
    pub deployment: std::sync::OnceLock<Arc<identity_keys::IdentityKeys>>,
}

impl IdentityAccess {
    /// Give the door the identity service's keys. Once.
    pub fn wire_identity_keys(&self, keys: Arc<identity_keys::IdentityKeys>) {
        let _ = self.deployment.set(keys);
    }

    /// The same, for a door being built.
    pub fn with_identity_keys(self, keys: Arc<identity_keys::IdentityKeys>) -> IdentityAccess {
        self.wire_identity_keys(keys);
        self
    }

    /// Give the door the second-step levels it asks of a session. Once: a
    /// second call is ignored, because the door may already be serving.
    pub fn wire_second_step(&self, levels: Arc<second_factors::SecondStepLevels>) {
        let _ = self.second_step.set(levels);
    }

    /// Tell the door the deployment's own space. Once.
    pub fn wire_console_home(&self, home: &str) {
        let _ = self.console_home.set(home.to_string());
    }

    /// The same, for a door being built.
    pub fn with_console_home(self, home: &str) -> IdentityAccess {
        self.wire_console_home(home);
        self
    }

    /// The ordinary constructor: a door with its own security-event
    /// recorder and its own metrics registry.
    ///
    /// Takes the engine by value and shares it. A deployment that also
    /// stands up `platform_ops::PlatformOps` wants the same `Arc` in both
    /// and should build it once — [`IdentityAccess::sharing`].
    pub fn new(
        engine: ordo_identity::AccessEngine,
        key: ordo_identity::TokenKey,
        now: Box<dyn Fn() -> u64 + Send + Sync>,
        metrics: Arc<metrics::Metrics>,
    ) -> IdentityAccess {
        IdentityAccess::sharing(Arc::new(engine), key, now, metrics)
    }

    /// The same, over an engine the caller already shares with something
    /// else — the platform ops, above all.
    pub fn sharing(
        engine: Arc<ordo_identity::AccessEngine>,
        key: ordo_identity::TokenKey,
        now: Box<dyn Fn() -> u64 + Send + Sync>,
        metrics: Arc<metrics::Metrics>,
    ) -> IdentityAccess {
        IdentityAccess {
            engine,
            key,
            now,
            stream: Arc::new(ordo_secevent::Recorder::default()),
            metrics,
            delegations: None,
            revocations: None,
            shares: None,
            second_step: std::sync::OnceLock::new(),
            console_home: std::sync::OnceLock::new(),
            deployment: std::sync::OnceLock::new(),
        }
    }

    /// Give the door the registry it asks about share grants. Builder-shaped
    /// for the reason [`IdentityAccess::with_delegations`] gives.
    pub fn with_shares(
        mut self,
        registry: Arc<dyn ordo_identity::grants::GrantRegistry>,
    ) -> IdentityAccess {
        self.shares = Some(registry);
        self
    }

    /// Give the door a delegation registry to ask.
    ///
    /// Builder-shaped rather than a fifth constructor argument because most
    /// deployments and every existing test have no delegations at all, and
    /// a required argument would have every one of them pass `None` — which
    /// reads as a decision nobody made.
    pub fn with_delegations(
        mut self,
        registry: Arc<dyn ordo_identity::delegation::DelegationRegistry>,
    ) -> IdentityAccess {
        self.delegations = Some(registry);
        self
    }

    /// Give the door a deny list to consult. Builder-shaped for the reason
    /// [`IdentityAccess::with_delegations`] gives.
    pub fn with_revocations(
        mut self,
        revocations: Arc<dyn ordo_identity::session::SessionRevocations>,
    ) -> IdentityAccess {
        self.revocations = Some(revocations);
        self
    }

    /// Drain the recorded events as operator-facing prose — the detail
    /// behind the one wire answer.
    ///
    /// Draining closes every open collapse window and **removes** the
    /// entries, so a deployment that also runs a
    /// [`crate::secdrain::SecurityDrain`] must not call this: the two
    /// split the entries between them, and what this one takes never
    /// reaches the durable stream. It is the drain for a process with no
    /// durable stream wired, and for tests.
    pub fn take_audit(&self) -> Vec<String> {
        self.stream
            .take_rendered(ordo_commandlog::Timestamp((self.now)()))
    }

    /// The security events themselves, drained. What a
    /// `SecurityStore` append takes. Same warning as
    /// [`IdentityAccess::take_audit`]: these have left the recorder.
    pub fn take_events(&self) -> Vec<ordo_secevent::StreamEntry> {
        self.stream
            .drain_all(ordo_commandlog::Timestamp((self.now)()))
    }
}

/// What the security record names as the "op" for a token-gate refusal.
///
/// The gate runs before any op is known — that is the property that makes
/// [`ordo_api::ErrorCode::DelegationRequired`] safe to distinguish — so
/// there is no op to name. An empty string would be recorded as
/// `<unprintable>` by [`ordo_secevent::sanitize_op`], which reads as a
/// hostile name rather than as an absent one; this says which gate refused,
/// in the character set the sanitizer admits.
pub const DELEGATION_GATE_OP: &str = "delegation-gate";

/// What the record names as the "op" for a refusal by the share gate, for
/// [`DELEGATION_GATE_OP`]'s reason.
pub const SHARE_GATE_OP: &str = "share-gate";

/// What the record names as the "op" when `GET /live` refuses a token that
/// does not verify, for [`DELEGATION_GATE_OP`]'s reason: a stream is opened
/// before any op is named.
pub const LIVE_GATE_OP: &str = "live-gate";

impl IdentityAccess {
    /// Was this verified token's session logged out, or were all of the
    /// person's sessions ended (a credential reset, a recovery)? The one
    /// session-revocation check: `check` (every op) and `token_known`
    /// (`GET /live`) both ask it. No list, or a token with no session,
    /// is not revoked (see the `revocations` field).
    fn session_ended(&self, claims: &ordo_identity::TokenClaims) -> bool {
        match (&self.revocations, claims.session.as_ref()) {
            (Some(list), Some(session)) => {
                list.is_revoked(&session.id)
                    || list.principal_ended(&claims.principal, session.started_at)
            }
            _ => false,
        }
    }

    /// Verify a deployment token with the identity service's keys. No keys
    /// wired is a refusal, `no-keys`.
    fn deployment_claims(
        &self,
        token: &str,
        now: u64,
    ) -> Result<
        ordo_identity::deployment_token::DeploymentClaims,
        ordo_identity::deployment_token::DeploymentTokenError,
    > {
        match self.deployment.get() {
            Some(keys) => keys.verify(token, now),
            None => Err(
                ordo_identity::deployment_token::DeploymentTokenError::Unverifiable(
                    ordo_identity::jose::JoseError("no-keys"),
                ),
            ),
        }
    }

    /// The touch a person's deployment token needs for `op` in the home
    /// space: the presence gate the engine holds for it, inside the
    /// deployment's window. `None` when the op asks none.
    fn deployment_touch_window(&self, op: &str) -> Option<u64> {
        match self.engine.step_up_for(op, None) {
            ordo_identity::StepUpRequirement::None => None,
            ordo_identity::StepUpRequirement::Presence { max_age_secs } => {
                Some(max_age_secs.min(self.engine.step_up_max_age_secs()))
            }
        }
    }

    /// The realm table's answer for a verified deployment token.
    fn deployment_realm(
        &self,
        claims: &ordo_identity::deployment_token::DeploymentClaims,
        space: &str,
        op: &str,
        now: u64,
    ) -> deployment_access::Realm {
        deployment_access::decide(
            claims,
            self.console_home.get().map(String::as_str),
            space,
            op,
            self.deployment_touch_window(op),
            now,
        )
    }

    /// The door's decision on a deployment token (`plan-identity-service.md`
    /// 4.3), recorded either way under the token's `sub`. A token that does
    /// not verify is recorded with no name and the reason.
    fn check_deployment(
        &self,
        token: &str,
        space: &str,
        op: &str,
        at: u64,
        stamp: &ordo_secevent::Stamp,
    ) -> AccessDecision {
        use deployment_access::Realm;
        use ordo_secevent::{RecordedDecision, Relationship, Subject};

        let cspace = ordo_commandlog::SpaceId(space.to_string());
        let claims = match self.deployment_claims(token, at) {
            Ok(c) => c,
            Err(e) => {
                self.stream.deny(
                    Some(&cspace),
                    Subject::Unverified,
                    op,
                    deployment_access::refused_as(&e),
                    stamp,
                    Relationship::None,
                );
                self.metrics.deny(space, "unknown", "unverified", op);
                return AccessDecision::Deny;
            }
        };
        let kind = deployment_access::metric_kind(&claims);
        let subject = deployment_access::subject(&claims);
        let refused = |decision: RecordedDecision| {
            self.stream.deny(
                Some(&cspace),
                subject.clone(),
                op,
                decision,
                stamp,
                Relationship::None,
            );
            self.metrics.deny(space, &claims.sub, kind, op);
            AccessDecision::Deny
        };
        match self.deployment_realm(&claims, space, op, at) {
            Realm::Allow => {
                self.stream.observe(
                    ordo_secevent::Observation {
                        space: Some(cspace.clone()),
                        subject: subject.clone(),
                        op: op.to_string(),
                        kind: ordo_secevent::EventKind::Authenticated,
                        at: stamp.at,
                        correlation: stamp.correlation.clone(),
                    },
                    Relationship::None,
                );
                AccessDecision::allow(claims.sub.clone())
            }
            Realm::PresenceRequired => refused(RecordedDecision::StepUpStale),
            Realm::Deny => refused(RecordedDecision::OutsideDeploymentRealm),
        }
    }
}

#[async_trait::async_trait]
impl AccessChecker for IdentityAccess {
    /// The registry gate, ahead of everything the op or the space decides.
    ///
    /// Three things it deliberately does not do:
    ///
    /// - **It does not verify the token's signature to decide the answer.**
    ///   It does verify it, because an unverifiable token has claimed
    ///   nothing and must fall through to the ordinary door (which will
    ///   refuse it as `access_denied`, the same as every other bad token —
    ///   answering `delegation_required` for a forged string would let a
    ///   stranger tell a signed token from an unsigned one).
    /// - **It does not look at the space.** The delegation names its own,
    ///   and a mismatch between that and the request's is
    ///   `DelegationSpaceMismatch` at the door, under the uniform answer —
    ///   because *that* one does depend on what the caller sent.
    /// - **It does not record a metric per token.** A denial here is
    ///   recorded in the security stream against the principal, which is
    ///   proven; a series keyed on the delegation id would let a caller
    ///   mint unbounded series by guessing.
    async fn token_gate(&self, token: &str) -> ordo_api::TokenGate {
        use ordo_secevent::{RecordedDecision, Subject};

        // A deployment token: fetch the identity service's keys once if it
        // names one the door does not hold, then let `check` decide. It
        // claims no delegation and no share.
        if ordo_identity::deployment_token::is_deployment_shape(token) {
            if let Some(keys) = self.deployment.get() {
                keys.prepare(token).await;
            }
            return ordo_api::TokenGate::Pass;
        }
        let at = (self.now)();
        let now = ordo_identity::Timestamp(at);
        let Ok(claims) = ordo_identity::verify(&self.key, token, now) else {
            return ordo_api::TokenGate::Pass;
        };
        // A share grant is asked about first, whatever else the token claims.
        if let Some(grant) = claims
            .capability
            .as_ref()
            .filter(|g| g.id().0.starts_with(crate::share_ops::GRANT_PREFIX))
        {
            let live = match &self.shares {
                Some(reg) => match reg.is_live(grant.space(), grant.id(), now).await {
                    Ok(live) => live,
                    // Unread is not "not live": the door cannot say, so the
                    // caller is told to retry (reported fix
                    // pool-exhaustion-reads-as-denied).
                    Err(_) => return ordo_api::TokenGate::Busy,
                },
                None => false,
            };
            if !live {
                let stamp = ordo_secevent::Stamp::new(
                    ordo_commandlog::Timestamp(at),
                    ordo_secevent::correlation::mint(),
                );
                let cspace = ordo_commandlog::SpaceId(grant.space().0.clone());
                self.stream.deny(
                    Some(&cspace),
                    Subject::capability(&claims.principal.0),
                    SHARE_GATE_OP,
                    RecordedDecision::ShareNotLive,
                    &stamp,
                    ordo_secevent::relationship(&self.engine, &claims, &cspace),
                );
                self.metrics.deny(
                    &grant.space().0,
                    &claims.principal.0,
                    "shared",
                    SHARE_GATE_OP,
                );
                return ordo_api::TokenGate::ShareNotLive;
            }
        }
        let Some(d) = claims.delegation.clone() else {
            // §8: without a delegation an agent "acts only as itself".
            // Not a refusal — an ordinary call.
            return ordo_api::TokenGate::Pass;
        };
        // A deployment with no registry cannot confirm any delegation, so it
        // confirms none. See the field's doc for why that is the honest
        // reading rather than a hole.
        let live = match &self.delegations {
            // An unreachable registry admits nothing and denies nothing: the
            // caller is told to retry (reported fix
            // pool-exhaustion-reads-as-denied).
            Some(reg) => match reg.is_live(&d.space, &d.id, now).await {
                Ok(live) => live,
                Err(_) => return ordo_api::TokenGate::Busy,
            },
            None => false,
        };
        if live {
            return ordo_api::TokenGate::Pass;
        }
        let stamp = ordo_secevent::Stamp::new(
            ordo_commandlog::Timestamp(at),
            ordo_secevent::correlation::mint(),
        );
        let cspace = ordo_commandlog::SpaceId(d.space.0.clone());
        // The op is unknown here by construction — that is the property that
        // makes this class safe to distinguish — so the record names the
        // gate rather than inventing an op the caller never got to.
        self.stream.deny(
            Some(&cspace),
            Subject::capability(&claims.principal.0),
            DELEGATION_GATE_OP,
            RecordedDecision::DelegationNotLive,
            &stamp,
            ordo_secevent::relationship(&self.engine, &claims, &cspace),
        );
        self.metrics.deny(
            &d.space.0,
            &claims.principal.0,
            "delegated",
            DELEGATION_GATE_OP,
        );
        ordo_api::TokenGate::DelegationNotLive
    }

    /// Whether the token verifies under this deployment's key, for the one
    /// place the door holds a connection before it can run [`Self::check`]
    /// (`GET /live`). Records the refusal as `check` would for an
    /// unverifiable token, under the gate's own name: there is no op yet.
    /// The deployment's own space: where a visitor's call to a public op
    /// runs (`design-public-path.md` §1). Public data lives there and
    /// nowhere else, which is what keeps every tenant out of reach of a
    /// caller with no token.
    fn anonymous_space(&self) -> Option<String> {
        Some(crate::provision::SYSTEM_SPACE.to_string())
    }

    fn caller_kind(&self, token: &str) -> ordo_api::CallerKind {
        let now = ordo_identity::Timestamp((self.now)());
        match ordo_identity::verify(&self.key, token, now) {
            Ok(c) if c.delegation.is_some() => ordo_api::CallerKind::Delegated,
            Ok(c) if c.capability.is_some() => ordo_api::CallerKind::Guest,
            _ => ordo_api::CallerKind::Person,
        }
    }

    /// A refusal the door made before `check` (no token, an unregistered
    /// op, a download that is off or closed to this caller), recorded as
    /// `check` records its own: to the space's stream when the token proves
    /// a principal related to the space, else the deployment's, and in the
    /// denial metric.
    fn record_refusal(
        &self,
        token: Option<&str>,
        space: Option<&str>,
        op: &str,
        why: ordo_api::DoorRefusal,
    ) {
        use ordo_api::DoorRefusal;
        use ordo_secevent::{RecordedDecision, Relationship, Subject};

        let at = (self.now)();
        let stamp = ordo_secevent::Stamp::new(
            ordo_commandlog::Timestamp(at),
            ordo_secevent::correlation::mint(),
        );
        let decision = match why {
            DoorRefusal::TokenMissing => RecordedDecision::TokenMissing,
            DoorRefusal::UnregisteredOp => RecordedDecision::UnregisteredOp,
            DoorRefusal::DownloadOff => RecordedDecision::DownloadOff,
            DoorRefusal::DownloadNotForCaller => RecordedDecision::DownloadNotForCaller,
        };
        let cspace = space.map(|s| ordo_commandlog::SpaceId(s.to_string()));
        // A deployment token's holder, named by its `sub` when it verifies.
        if let Some(t) = token.filter(|t| ordo_identity::deployment_token::is_deployment_shape(t)) {
            match self.deployment_claims(t, at) {
                Ok(c) => {
                    self.stream.deny(
                        cspace.as_ref(),
                        deployment_access::subject(&c),
                        op,
                        decision,
                        &stamp,
                        Relationship::None,
                    );
                    self.metrics.deny(
                        space.unwrap_or(""),
                        &c.sub,
                        deployment_access::metric_kind(&c),
                        op,
                    );
                }
                Err(e) => {
                    self.stream.deny(
                        cspace.as_ref(),
                        Subject::Unverified,
                        op,
                        deployment_access::refused_as(&e),
                        &stamp,
                        Relationship::None,
                    );
                    self.metrics
                        .deny(space.unwrap_or(""), "unknown", "unverified", op);
                }
            }
            return;
        }
        let claims = token
            .and_then(|t| ordo_identity::verify(&self.key, t, ordo_identity::Timestamp(at)).ok());
        match claims {
            Some(claims) => {
                let (subject, kind) = if claims.capability.is_some() {
                    (Subject::capability(&claims.principal.0), "shared")
                } else if claims.delegation.is_some() {
                    (Subject::capability(&claims.principal.0), "delegated")
                } else {
                    (Subject::member(&claims.principal.0), "member")
                };
                let rel = cspace.as_ref().map_or(Relationship::None, |c| {
                    ordo_secevent::relationship(&self.engine, &claims, c)
                });
                self.stream
                    .deny(cspace.as_ref(), subject, op, decision, &stamp, rel);
                self.metrics
                    .deny(space.unwrap_or(""), &claims.principal.0, kind, op);
            }
            None => {
                // Nothing proven: the deployment's stream, with no name, as
                // `check` records an unverifiable token.
                let decision = match (token, why) {
                    (Some(_), DoorRefusal::TokenMissing) => RecordedDecision::TokenUnverifiable,
                    _ => decision,
                };
                self.stream.deny(
                    cspace.as_ref(),
                    Subject::Unverified,
                    op,
                    decision,
                    &stamp,
                    Relationship::None,
                );
                let kind = if token.is_some() {
                    "unverified"
                } else {
                    "anonymous"
                };
                self.metrics.deny(space.unwrap_or(""), "unknown", kind, op);
            }
        }
    }

    fn token_known(&self, token: &str) -> bool {
        use ordo_secevent::{RecordedDecision, Relationship, Subject};

        let at = (self.now)();
        // A deployment token opens no live stream: a stream carries app
        // data, which is outside the deployment realm.
        if ordo_identity::deployment_token::is_deployment_shape(token) {
            let stamp = ordo_secevent::Stamp::new(
                ordo_commandlog::Timestamp(at),
                ordo_secevent::correlation::mint(),
            );
            let (subject, decision, who, kind) = match self.deployment_claims(token, at) {
                Ok(c) => (
                    deployment_access::subject(&c),
                    RecordedDecision::OutsideDeploymentRealm,
                    c.sub.clone(),
                    deployment_access::metric_kind(&c),
                ),
                Err(e) => (
                    Subject::Unverified,
                    deployment_access::refused_as(&e),
                    "unknown".to_string(),
                    "unverified",
                ),
            };
            self.stream.deny(
                None,
                subject,
                LIVE_GATE_OP,
                decision,
                &stamp,
                Relationship::None,
            );
            self.metrics.deny("", &who, kind, LIVE_GATE_OP);
            return false;
        }
        if let Ok(claims) = ordo_identity::verify(&self.key, token, ordo_identity::Timestamp(at)) {
            // The same session check every op call runs (`session_ended`):
            // a logged-out, revoked or replayed session opens no stream.
            if !self.session_ended(&claims) {
                return true;
            }
            let stamp = ordo_secevent::Stamp::new(
                ordo_commandlog::Timestamp(at),
                ordo_secevent::correlation::mint(),
            );
            self.stream.deny(
                None,
                Subject::member(&claims.principal.0),
                LIVE_GATE_OP,
                RecordedDecision::SessionRevoked,
                &stamp,
                Relationship::None,
            );
            self.metrics
                .deny("", &claims.principal.0, "member", LIVE_GATE_OP);
            return false;
        }
        let stamp = ordo_secevent::Stamp::new(
            ordo_commandlog::Timestamp(at),
            ordo_secevent::correlation::mint(),
        );
        self.stream.deny(
            None,
            Subject::Unverified,
            LIVE_GATE_OP,
            RecordedDecision::TokenUnverifiable,
            &stamp,
            Relationship::None,
        );
        self.metrics.deny("", "unknown", "unverified", LIVE_GATE_OP);
        false
    }

    /// A deployment person whose touch is too old for `op`: the realm
    /// table, asked again after `check` denied.
    fn presence_required(&self, token: &str, space: &str, op: &str) -> bool {
        if !ordo_identity::deployment_token::is_deployment_shape(token) {
            return false;
        }
        let at = (self.now)();
        self.deployment_claims(token, at).is_ok_and(|c| {
            self.deployment_realm(&c, space, op, at) == deployment_access::Realm::PresenceRequired
        })
    }

    fn check(&self, token: &str, space: &str, op: &str) -> AccessDecision {
        use ordo_identity::access::Decision;
        use ordo_identity::principal::{SpaceId, Timestamp};
        use ordo_secevent::{RecordedDecision, Relationship, Subject};

        let at = (self.now)();
        let now = Timestamp(at);
        // One correlation per door decision. It is the value a command group
        // for this request would carry, so the security entry and the chain
        // group join on it — see `ordo_secevent::correlation` for what that
        // reaches today and what it will reach when the trace and the
        // structured request log exist.
        let stamp = ordo_secevent::Stamp::new(
            ordo_commandlog::Timestamp(at),
            ordo_secevent::correlation::mint(),
        );
        // **A deployment token** (3 segments) is the identity service's, and
        // the realm table decides it (`plan-identity-service.md` 4.3). It
        // never reaches the HMAC verifier, and an app token never reaches
        // this branch.
        if ordo_identity::deployment_token::is_deployment_shape(token) {
            return self.check_deployment(token, space, op, at, &stamp);
        }
        let cspace = ordo_commandlog::SpaceId(space.to_string());
        let claims = match ordo_identity::verify(&self.key, token, now) {
            Ok(c) => c,
            Err(_) => {
                // Nothing to attribute: the token did not prove a principal.
                // The record says `Unverified` and carries no name, no
                // claimed subject and no token fingerprint — inventing an
                // attribution is worse than admitting there is none. The
                // metric agrees, in one bucket: a per-token series would let
                // a caller mint unbounded series by guessing.
                //
                // `Relationship::None` is the truthful input and also the
                // only safe one; `route` would send an unverified subject to
                // the deployment stream whatever were passed here, which is
                // the property that stops a stranger growing a space's
                // audit spine.
                self.stream.deny(
                    Some(&cspace),
                    Subject::Unverified,
                    op,
                    RecordedDecision::TokenUnverifiable,
                    &stamp,
                    Relationship::None,
                );
                self.metrics.deny(space, "unknown", "unverified", op);
                return AccessDecision::Deny;
            }
        };
        // **The logged-out token**, refused before the engine is asked
        // anything. Placed here and not earlier for one reason: the check
        // needs a session id, which only a verified token has. Placed here
        // and not later because a revoked session must be refused whatever
        // the op is, including an op the principal would otherwise be
        // allowed: the whole point of a logout is that it does not depend
        // on what is being asked for.
        //
        // The record names the session and not the principal in its op
        // field for the same reason the delegation gate names itself: this
        // is a gate, and `op` is the op the caller was reaching for, which
        // is the useful half.
        if self.session_ended(&claims) {
            self.stream.deny(
                Some(&cspace),
                Subject::member(&claims.principal.0),
                op,
                RecordedDecision::SessionRevoked,
                &stamp,
                ordo_secevent::relationship(&self.engine, &claims, &cspace),
            );
            self.metrics.deny(space, &claims.principal.0, "member", op);
            return AccessDecision::Deny;
        }
        // **A console key session reaches nothing while the door verifies
        // the identity service's tokens**: the console then calls the door
        // as the signed-in person, so a session a console key opened before
        // `ORDO_IDENTITY_URL` was set ends here (`plan-identity-service.md`
        // 6, G14).
        if self.deployment.get().is_some() && crate::console_keys::is_console_session(&claims) {
            self.stream.deny(
                Some(&cspace),
                Subject::member(&claims.principal.0),
                op,
                RecordedDecision::ConsoleKeyClosed,
                &stamp,
                ordo_secevent::relationship(&self.engine, &claims, &cspace),
            );
            self.metrics.deny(space, &claims.principal.0, "member", op);
            return AccessDecision::Deny;
        }
        // **A console key session reaches the deployment's own space**, and
        // a tenant's only for its exit and bucket ops with a fresh touch
        // (`console_keys::console_may`, the one check). Its token names no
        // space, so the door binds it here, before the engine.
        if crate::console_keys::console_may(
            &claims,
            crate::console_keys::Reach::Space {
                space,
                home: self.console_home.get().map(String::as_str),
                op,
                now: at,
            },
        )
        .is_err()
        {
            self.stream.deny(
                Some(&cspace),
                Subject::member(&claims.principal.0),
                op,
                RecordedDecision::ConsoleSessionOffHome,
                &stamp,
                ordo_secevent::relationship(&self.engine, &claims, &cspace),
            );
            self.metrics.deny(space, &claims.principal.0, "member", op);
            return AccessDecision::Deny;
        }
        let ispace = SpaceId(space.to_string());
        match self.engine.check(&claims, &ispace, op, now) {
            // **`$caller` is the member in this space** (E1): the row id the
            // engine holds for this person here. A token with no membership
            // that the engine still allowed is a capability token, and it
            // stays the principal it always was, which no guard can match to
            // a members row.
            Decision::Allow => {
                // **A tenant that requires a second step refuses a session
                // that skipped it.** A person who signed in without naming the
                // tenant met no requirement then, and meets it here, beside
                // the revocation check, for every op. Recorded with its own
                // decision so the tenant's owner can tell "this person should
                // sign in again naming us" from a permission problem.
                if self
                    .second_step
                    .get()
                    .is_some_and(|levels| levels.refuses(&claims, space))
                {
                    self.stream.deny(
                        Some(&cspace),
                        Subject::member(&claims.principal.0),
                        op,
                        RecordedDecision::SecondStepRequired,
                        &stamp,
                        ordo_secevent::relationship(&self.engine, &claims, &cspace),
                    );
                    self.metrics.deny(space, &claims.principal.0, "member", op);
                    return AccessDecision::Deny;
                }
                // **Under a delegation `$caller` is the delegator**
                // (`plan-delegation-surface.md` §7, increment 2). The engine
                // has just bounded this call by the delegator's live reach,
                // so the row guards answer for the same person: the agent
                // sees the delegator's rows and not one row more. The
                // agent rides beside, for the command log alone.
                let member = |p: &ordo_identity::principal::PrincipalId| {
                    self.engine
                        .member_id_of(&ispace, p)
                        .map(str::to_string)
                        .unwrap_or_else(|| p.0.clone())
                };
                match &claims.delegation {
                    Some(d) => AccessDecision::Allow {
                        principal: member(&d.delegator),
                        agent: Some(member(&claims.principal)),
                    },
                    None => AccessDecision::allow(member(&claims.principal)),
                }
            }
            Decision::Deny { op, reason } => {
                let capability = claims.capability.is_some();
                // The **decision**, not the wire code: the wire answers one
                // uniform `access_denied` under non-enumerability, so every
                // distinction it erases has to survive here or the two
                // surfaces would be the same surface.
                let decision = RecordedDecision::from(&reason);
                // Members only. A denial reaches the space's own stream only
                // when this principal has a relationship with it; everyone
                // else lands in the deployment stream, where the operator's
                // abuse alerting lives.
                let rel = ordo_secevent::relationship(&self.engine, &claims, &cspace);
                self.stream.deny(
                    Some(&cspace),
                    if capability {
                        Subject::capability(&claims.principal.0)
                    } else {
                        Subject::member(&claims.principal.0)
                    },
                    &op,
                    decision,
                    &stamp,
                    rel,
                );
                // The principal is proven here, so the denial is
                // attributable — which is the whole point of the alert: a
                // sweep is one principal's denial rate across ops and
                // spaces, and it is invisible in any un-attributed count.
                self.metrics.deny(
                    space,
                    &claims.principal.0,
                    if capability { "capability" } else { "member" },
                    &op,
                );
                AccessDecision::Deny
            }
        }
    }
}

/// Build the deployment's access engine from the fixture: one membership
/// per platform member (their `role` column IS their role — the platform
/// entity is the directory), a profile per space (the app developer's
/// template, or reads to everyone and writes to editors and up where it
/// names nothing, with that space's own changes laid over it), and **every
/// op deployed with its Stage-8 manifest hash**.
///
/// That last part is the join that makes ops truth load-bearing: a
/// capability grant pins, **per op**, the manifest it consented to — its
/// hash, its field set, and its row-scope shape — so changing what an op
/// *touches or discloses* kills the grant until a human re-consents. A
/// guard that stopped applying is caught by the row-scope half, which is the
/// half `security-logging-design.md` §5's field-set-only form did not have
/// and which `definition-language.md` §8 shows is the one that matters.
/// Without it the catalog is a document; with it, it is an enforcement
/// input.
///
/// # Every space in the directory, not one
///
/// The membership query used to read `SELECT "id", "role" FROM "members"`
/// with no space predicate and grant every row it found membership in the
/// one space this host served. With one space that was merely redundant.
/// With many it is a cross-tenant grant: a principal whose only `members`
/// row is in space B would have been made a member of space A, at whatever
/// role B gave them, by the door itself. The predicate is now the `$space`
/// column and the loop sets a membership per `(space, principal)` and a
/// profile per space.
///
/// Each space's profile starts as the deployment's (the app developer's
/// template, [`access_template`]) and carries what that space changed
/// ([`profile_ops`], 27ah): a space loosening `std.E.get` to its viewers
/// (std-library §5 rule 3) is a profile command in that space alone.
///
/// Built from the table, so a space [`provision::provision_space`] created
/// is a space the next engine built from this function serves, with no
/// second path to registering one.
pub fn deployment_engine(f: &Fixture, pg: &Pg) -> Result<ordo_identity::AccessEngine, String> {
    // Directory: the platform's own members entity, read unguarded because
    // this IS the door, running before any caller is established. Ordered
    // by space then id so a rebuild is deterministic across every space it
    // finds rather than only within one.
    let rows = pg.query(MEMBERS_SQL)?;
    let tenant = profile_ops::TenantAccess::read_pg(pg)?;
    engine_from_directory(
        f,
        &rows,
        &tenant,
        &presence::current(),
        &pause::current(),
        &delegable::current(),
        &access_template::current(),
    )
}

/// [`deployment_engine`] under an explicit delegable-ops policy
/// ([`delegable`]), in place of the one the deployment installed. For a
/// test that opens a delete to delegation without touching the process.
pub fn deployment_engine_delegating(
    f: &Fixture,
    pg: &Pg,
    delegating: &delegable::DelegablePolicy,
) -> Result<ordo_identity::AccessEngine, String> {
    let rows = pg.query(MEMBERS_SQL)?;
    let tenant = profile_ops::TenantAccess::read_pg(pg)?;
    engine_from_directory(
        f,
        &rows,
        &tenant,
        &presence::current(),
        &pause::current(),
        delegating,
        &access_template::current(),
    )
}

/// The statement every directory reads `members` with, in one place so the
/// single-database and the routed directory cannot select different columns.
///
/// **`status` is the fourth column, and until 2026-09-28 it was not read.**
/// `definition-language.md` gives every member a `status` of `active` or
/// `suspended` and says *"suspension is a command, not a delete"*;
/// `AccessEngine::suspend` implements it; and this statement selected three
/// columns, so every member loaded as active and the engine's suspension had
/// no caller outside its own tests (`security-review-6.md` S6-002, verified
/// against the running binary). New columns go at the END: every reader
/// indexes by position.
///
/// **`principal` is the fifth column (E1, 2026-09-29).** The door resolves a
/// verified person and the tenant a request names to the member row whose
/// `principal` is that person, so one person is a different member in each
/// tenant. Before this the column was written and never read, and a token
/// for principal `usr_abc` was looked up among memberships keyed `mem_...`.
///
/// **`kind` is the sixth column.** A group is a `members` row with `kind:
/// group` (DL §3 Groups); it is a set of people, never a person who holds a
/// token, so the door skips it as a membership and reads its links from
/// `groupMembers` instead ([`profile_ops::TenantAccess`]).
pub const MEMBERS_SQL: &str = r#"SELECT "$space", "id", "role", "status", "principal", "kind" FROM "members" ORDER BY "$space", "id""#;

/// The same engine, over the **pool** rather than over a `psql` fork.
///
/// This is what a deployable mounts, and the difference is not tidiness.
/// [`deployment_engine`]'s one directory read goes through
/// `ordo_conformance::Pg`, which executes by running the `psql` *binary*:
/// that would put an apt feed into the image that serves customer data, and
/// it is a blocking process spawn on every rebuild of the engine. Everything
/// else — the role mapping, the per-role profile, std ops being
/// registry-present and exposure-opt-in, the `deploy_op` manifest pins, the
/// reserved platform-op grants — is one body, shared, because that body is
/// where `std-library.md` §5's rules 2 and 3 are implemented and where the
/// capability pins that make ops truth enforceable are installed. Two
/// copies of it would be two answers waiting to differ about who may call
/// what.
///
/// [`deployment_engine`] is not deleted: the conformance suites call it
/// with a `Pg`, and rewriting them is not this change.
///
/// **The engine is built once and rebuilt on a swap**, so a membership
/// change takes effect at the next swap or the next restart rather than on
/// the next request. That is weaker than `IdentityAccess`'s own doc reads,
/// and the sentence there is true of its `now`-driven checks rather than of
/// this table. A refresh on the invalidation bus is the obvious fix and it
/// belongs to `ordo-identity`'s ownership of that state, not to the binary.
pub async fn deployment_engine_pooled(
    f: &Fixture,
    pool: &ordo_exec::Pool,
) -> Result<ordo_identity::AccessEngine, String> {
    // **A database nobody has run `ordo apply` against has no directory
    // table**, and that is not a failure to route around: the binary boots
    // into a no-program state (`Deployment::empty`) precisely so a fresh
    // deployment can be deployed to, and `members` is created by a
    // program's DDL. Asking the *schema* would not answer it — `members` is
    // a platform entity `ordo_schema` injects into every program, including
    // the empty one — so the question goes where the answer is.
    //
    // `to_regclass` rather than matching the error text, and the difference
    // is the whole point: a directory table that does not exist is an
    // un-applied deployment; a directory table that exists and will not
    // read is a failure, and stays one.
    //
    // With no directory the engine is built over an empty membership list,
    // which denies every token. Correct: there is no program to call
    // anything in, and `/health/ready` already names the two checks it
    // cannot pass.
    let applied = pool
        .rows(r#"SELECT to_regclass('"members"') IS NOT NULL"#)
        .await
        .map_err(|e| format!("members: {e}"))?;
    let applied = matches!(
        applied.first().and_then(|r| r[0].as_deref()),
        Some("t") | Some("true")
    );
    let (rows, tenant) = if applied {
        (
            pool.rows(MEMBERS_SQL)
                .await
                .map_err(|e| format!("members: {e}"))?,
            profile_ops::TenantAccess::read(pool)
                .await
                .map_err(|e| format!("profiles: {e}"))?,
        )
    } else {
        (Vec::new(), profile_ops::TenantAccess::default())
    };
    engine_from_directory(
        f,
        &rows,
        &tenant,
        &presence::current(),
        &pause::current(),
        &delegable::current(),
        &access_template::current(),
    )
}

/// The access engine, from `members` rows (`$space`, `id`, `role`, `status`,
/// `principal`) wherever
/// they were read: one database in [`deployment_engine`], or each space's own
/// database in [`directory::routed_engine`].
pub fn engine_from_members(
    f: &Fixture,
    rows: &[Vec<Option<String>>],
) -> Result<ordo_identity::AccessEngine, String> {
    engine_from_members_under(f, rows, &presence::current())
}

/// [`engine_from_members`] under an explicit presence policy: which platform
/// ops require a fresh passkey assertion. The one the deployment installed
/// is what [`engine_from_members`] uses, so every rebuild of the door
/// (a deploy, a member change, the refresh timer) agrees.
pub fn engine_from_members_under(
    f: &Fixture,
    rows: &[Vec<Option<String>>],
    policy: &presence::PresencePolicy,
) -> Result<ordo_identity::AccessEngine, String> {
    engine_from_members_with(f, rows, policy, &pause::current())
}

/// [`engine_from_members_under`] with an explicit pause policy as well: which
/// tenant roles hold `std.space.pause` and `std.space.resume` (27ar). The
/// installed one is what every rebuild of the door uses.
pub fn engine_from_members_with(
    f: &Fixture,
    rows: &[Vec<Option<String>>],
    policy: &presence::PresencePolicy,
    pausing: &pause::PausePolicy,
) -> Result<ordo_identity::AccessEngine, String> {
    engine_from_members_delegating(f, rows, policy, pausing, &delegable::current())
}

/// [`engine_from_members_with`] with an explicit delegable-ops policy as
/// well: which delete ops the app developer opened to delegated tokens
/// (SH-20, checklist 11-4d). The installed one is what every rebuild of the
/// door uses.
pub fn engine_from_members_delegating(
    f: &Fixture,
    rows: &[Vec<Option<String>>],
    policy: &presence::PresencePolicy,
    pausing: &pause::PausePolicy,
    delegating: &delegable::DelegablePolicy,
) -> Result<ordo_identity::AccessEngine, String> {
    engine_from_directory(
        f,
        rows,
        &profile_ops::TenantAccess::default(),
        policy,
        pausing,
        delegating,
        &access_template::current(),
    )
}

/// The whole door, from everything it reads: the `members` rows, what each
/// tenant changed about its profile and who is in its groups (`tenant`),
/// the deployment's policies, and the app developer's profile `template`.
///
/// Each tenant's profile is the deployment's profile with that tenant's own
/// lines laid over it ([`profile_ops::tenant_profile`]), so a tenant's change
/// reaches that tenant alone.
pub fn engine_from_directory(
    f: &Fixture,
    rows: &[Vec<Option<String>>],
    tenant: &profile_ops::TenantAccess,
    policy: &presence::PresencePolicy,
    pausing: &pause::PausePolicy,
    delegating: &delegable::DelegablePolicy,
    template: &access_template::AccessTemplate,
) -> Result<ordo_identity::AccessEngine, String> {
    use ordo_identity::principal::{AccessProfile, PrincipalId, Role, SpaceId};

    let mut engine = ordo_identity::AccessEngine::new();
    // A provider's two-step claim counts where the app developer and the
    // tenants said so (`plan-auth-federated.md` §8.2). Nothing installed is
    // nowhere.
    if let Some(acceptance) = presence::upstream() {
        engine.accept_upstream(acceptance);
    }
    let mut spaces: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut profile = AccessProfile::new();
    for r in rows {
        let (Some(space), Some(id), role) = (r[0].clone(), r[1].clone(), r[2].clone()) else {
            continue;
        };
        // A group is a set of people, not a member who signs in.
        if r.get(5).and_then(|k| k.as_deref()) == Some("group") {
            continue;
        }
        let role = match role.as_deref() {
            Some("owner") => Some(Role::Owner),
            Some("manager") => Some(Role::Manager),
            Some("editor") => Some(Role::Editor),
            Some("viewer") => Some(Role::Viewer),
            // No role is a **guest**: a seat that reaches only what a share
            // grant lists (`plan-sharing.md` §3.1). It used to read as a
            // viewer, which let a seat minted for one shared row call every
            // read op the viewer profile lists. An unrecognised word is
            // treated the same way, the narrower reading: a role this build
            // has never heard of must not widen into a viewer.
            _ => None,
        };
        // **The person, not the row.** A member is keyed by the principal the
        // `principal` column names, and carries its own row id, which is what
        // `$caller` binds to (E1). A row with no principal, or a caller
        // handing over the old four-column shape, is keyed by its id: that
        // is the member id standing in as the subject, which is what tests
        // and `ordo dev` mint their tokens with.
        // A seat that has **left** is no membership at all (27aq), and nor is
        // one whose person then deleted their account (`deleted`). Its row
        // stays so the rows that point at it keep a target, and the door
        // gives it nothing: not a suspension a reinstate could undo.
        if r.get(3)
            .and_then(|s| s.as_deref())
            .is_some_and(crate::release_ops::is_gone)
        {
            continue;
        }
        let principal = PrincipalId(
            r.get(4)
                .and_then(|p| p.clone())
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| id.clone()),
        );
        engine.set_member(&SpaceId(space.clone()), &principal, &id, role);
        // Suspended, and only on the exact word. Anything else, including a
        // status this build has never heard of, leaves the member active:
        // the column is the platform's own and a value outside its two is a
        // defect to surface elsewhere, where refusing a whole space's
        // members over it would be an outage caused by a typo. `get(3)`
        // rather than `[3]` so a caller handing over the old three-column
        // rows degrades to the old behaviour instead of panicking the door.
        if r.get(3).and_then(|s| s.as_deref()) == Some("suspended") {
            engine.suspend(&SpaceId(space.clone()), &principal);
        }
        spaces.insert(space);
    }

    // Profile: reads for every role, writes for editor and up. Deliberately
    // explicit per role — `Role::level` is a level, not a hierarchy, so
    // there is no inheritance to lean on.
    let specs = build_specs(f);
    for s in &specs {
        let writes = !matches!(s.kind, OpKind::Read);
        // std ops are **registry-present, exposure opt-in**
        // (`std-library.md` §5 rules 2 and 3): deriving an op and exposing
        // it are different acts. This template is the fixture's profile,
        // so rule 3 is what it must implement — std reaches owner and
        // manager, and viewer/editor access to a whole derived row is a
        // per-space loosening a manager performs deliberately.
        //
        // Without this the blanket grant below would hand every viewer the
        // entire std surface of every entity the moment std derived — one
        // deploy turning a fixture convenience into the largest privilege
        // escalation in the system. The rule is cheap to state and the
        // failure it prevents is not recoverable by noticing later.
        // The app developer's default for this op, when the template names
        // it, in place of the built-in one (`access_template`).
        for role in access_template::default_roles(template, &s.name, writes) {
            profile = profile.allow(role, &s.name);
        }
        // Deploy-time: the op's manifest hash, from the same manifest the
        // catalog publishes.
        if let Some(pin) = op_pin(f, &s.name) {
            engine.deploy_op(&s.name, pin);
        }
    }

    // The deployment's own surfaces. Not registered ops — reserved names in
    // the same namespace, so who may see the deployment's internals is
    // decided by the access model that already exists rather than by a
    // second, parallel notion of "operator" with its own credential.
    //
    // Owner only, and for a stated reason each: `/health/deep` names the
    // KeyProvider and the database, `/metrics` carries principal ids
    // and op names across every space this process served, and
    // `/security/alerts` names which principals this process's judgement
    // says look like they are probing. A manager who may edit data has no
    // business reading any of them.
    // **The blob door's read gate**, and it is here because it was
    // nowhere: `GET /blob/{ref}` checks `std.$blob.get`, nothing granted
    // it, and so every blob read answered `access_denied` for every
    // principal — with the row guard inside `PlatformBlobs::read`
    // unreachable behind it. Found by the first test to drive that route
    // through a real deployment.
    //
    // **Manager and owner**, which is the std exposure rule above applied
    // rather than excepted. Reading a file is reading part of a row, and
    // rule 3 says viewer/editor access to a derived row is a per-space
    // loosening a manager performs deliberately; a blob read that reached
    // viewers by default would be a way around that rule for exactly the
    // rows most worth protecting.
    //
    // The residual is worth stating rather than discovering. A viewer who
    // can read a row through an **authored** op still cannot read its
    // file, because authored ops are granted individually and this gate is
    // not one of them. That fails closed, which is the right direction to
    // be wrong in, and it is the same loosening `std.E.get` already needs
    // — but an app whose viewers must see avatars has to perform it, and
    // that is a fact its author should meet here rather than in support.
    //
    // There is no write counterpart: `PUT /blob/{entity}/{field}/{id}`
    // names its entity in the URL, so it checks that entity's own
    // `std.E.update` and inherits the rule with nothing to grant.
    for op in [ordo_api::blob::BLOB_READ_OP] {
        for role in [Role::Manager, Role::Owner] {
            profile = profile.allow(role, op);
        }
        engine.deploy_op(
            op,
            ordo_identity::OpPin {
                manifest_hash: ordo_identity::principal::ManifestHash::of(op.as_bytes()),
                unguarded: false,
                entities: Vec::new(),
            },
        );
    }

    // **`SECEVENTS_OP` is in this list and is not like the others.**
    //
    // The profile this function builds is deployment-wide policy installed
    // in **every** space, so `Role::Owner` here means "the owner of whatever
    // space the request is bound to". For the others that is the
    // deployment's own space, because `serve::authorize` binds it. For
    // `/security/events` it is the space the caller named, which is the
    // whole point of 7d(b): a tenant owner holds this op **in their own
    // space** and holds nothing in anybody else's, because they have no
    // members row there. So the ordinary access model answers "may this
    // person read this space's denials" with no second notion of operator.
    //
    // One consequence, stated because it is a policy choice and not a
    // leftover: a **deployment** operator does not thereby gain read of
    // every tenant's stream. That follows §6.1's split — the deployment's
    // own stream is the operator's artifact and a space's is its members' —
    // and an operator who needs a tenant's record reads it with `ordo
    // verify --space` at the database, which is an act on the host rather
    // than a route anyone can call.
    //
    // Owner-only rather than manager-and-owner, matching `/security/alerts`
    // and 7d's own done-when. The contrary argument is real and is the one
    // `callersOf` below makes — an audit surface its own administrator
    // cannot read pushes deployments to hand out owner — so this is the
    // conservative direction on a disclosure that names suspects, and is
    // worth revisiting rather than assumed settled.
    //
    // `USAGE_OP` joins them on `ALERTS_OP`'s argument (console step 7):
    // `serve::usage_route` authorizes against it in the deployment's own
    // space, and nothing deployed it, so `/usage` refused every caller of a
    // deployment built through this function, its owner included.
    for op in [
        serve::HEALTH_DEEP_OP,
        serve::METRICS_OP,
        serve::ALERTS_OP,
        serve::SECEVENTS_OP,
        serve::USAGE_OP,
    ] {
        profile = profile.allow(Role::Owner, op);
        // Deployed with a pin like every other op, so a capability grant
        // naming one dies if what it discloses changes.
        //
        // These are the deployment's own surfaces, not compiled ops: they
        // touch no entity, so the pin has an empty entity list and its hash
        // is over the op name. An empty shape is honest here — there is no
        // row bound to state, because there are no rows.
        //
        // `ALERTS_OP` joins the other two on the same argument rather than
        // on a new one: `serve::alerts_route` already authorizes against it
        // and nothing deployed it, so the route denied everyone in every
        // deployment built through this function (a mounted route with no
        // op behind it), found by D0's own test rather than by an operator
        // reaching for it during an incident.
        engine.deploy_op(
            op,
            ordo_identity::OpPin {
                manifest_hash: ordo_identity::principal::ManifestHash::of(op.as_bytes()),
                unguarded: false,
                entities: Vec::new(),
            },
        );
    }

    // 8r: the platform ops. Two rules, and both are argued in
    // `platform_ops`'s module doc.
    //
    // **Manager and owner, explicitly listed.** Roles carry no implicit
    // hierarchy, so "and above" has to be spelled. Not editor: an editor
    // cannot change who may call anything, so the answer informs no act
    // available to them and what they would gain is a map of the space's
    // privilege structure. Not owner-only: the manager is the role that
    // administers the answer, and an audit surface its own administrator
    // cannot read is one that pushes deployments to hand out owner.
    //
    // **Step-up.** `callersOf` returns every member who can call an op,
    // every group and override behind them, and every capability grant over
    // it — whose it is and which client it was consented to. That last part
    // is other people's consent records, and `require_step_up`'s own doc
    // names full export as what belongs here. §5 pins that capability
    // tokens can never satisfy step-up, so this is also the mechanism that
    // makes the op structurally human-only: Alice may delegate Alice's
    // authority, and Bob's connected apps are not hers to delegate. Reusing
    // step-up rather than minting a "human-only" flag matters — the
    // platform has exactly one mechanism for *not delegable*, and a second
    // would be a second thing to get right.
    for spec in platform_ops::specs() {
        // **A person's own answer is for every role and asks for no
        // presence.** `std.person.tenants` tells the caller which tenants
        // they belong to and nothing else, so it is no administration and
        // belongs to viewers too. It is handled here and skips the role
        // list and the step-up below, which are for ops that act on a space.
        if platform_ops::person_scoped(&spec.name) {
            for role in [Role::Viewer, Role::Editor, Role::Manager, Role::Owner] {
                profile = profile.allow(role, &spec.name);
            }
            engine.deploy_op(
                &spec.name,
                ordo_identity::OpPin {
                    manifest_hash: ordo_identity::principal::ManifestHash::of(
                        platform_ops::manifest(&spec).to_string().as_bytes(),
                    ),
                    unguarded: false,
                    entities: Vec::new(),
                },
            );
            continue;
        }
        // **The exit ops are the owner's alone.** `platform_ops::owner_only`
        // owns that decision so it sits beside the ops rather than in this
        // loop, where a role list gets loosened by whoever is next editing
        // the engine. Ending a space and disclosing all of it are not
        // administration: a manager administers who may do what, and these
        // two are the acts that end the thing being administered.
        //
        // Blanket-listing every platform op for `Manager` — which is what
        // this loop did while `callersOf` was the only one — would have made
        // `std.space.delete` manager-reachable the moment it was added, in
        // silence.
        // **Leaving is every member's own act** (27aq): any role may ask, and
        // the program's release rule decides. A guest has no role here and
        // keeps `/auth/share/leave`.
        // **Pausing from inside a tenant is the app developer's grant**
        // (27ar, owner 2026-10-05): the roles the pause policy names, and
        // by default none, which leaves the op to the deployment owner.
        let governed = crate::session_policy::TEMPLATE_GOVERNED.contains(&spec.name.as_str());
        // The tenant setter, when the template names it: its roles, and the
        // owner as well, so a closed setter is refused by the op with the
        // setting that opens it rather than by the door's one denial
        // (`session_policy::closed_by_template`).
        let with_owner: Option<Vec<Role>> = template
            .roles_for(&spec.name)
            .filter(|_| crate::session_policy::TENANT_SETTERS.contains(&spec.name.as_str()))
            .map(|r| {
                let mut v = r.to_vec();
                if !v.contains(&Role::Owner) {
                    v.push(Role::Owner);
                }
                v
            });
        let roles: &[Role] = if let Some(granted) = pausing.roles_for(&spec.name) {
            granted
        } else if let Some(granted) = with_owner.as_deref() {
            granted
        } else if let Some(granted) = template.roles_for(&spec.name).filter(|_| governed) {
            // The app developer's template, for the two session policy ops
            // it may name (`access_template`).
            granted
        } else {
            platform_ops::default_roles(&spec.name)
        };
        for role in roles {
            profile = profile.allow(*role, &spec.name);
        }
        // Presence, unless the app developer said this op does not need it
        // (`ORDO_STEP_UP_EXEMPT_OPS`, owner 2026-09-29). The default is that
        // every platform op does, so an op nobody named still asks.
        // The consent ceremony asks for presence at the approval alone:
        // reading a request, declining it, listing and revoking take
        // nothing from anyone but the agent (11-4b).
        let consent_free =
            crate::consent::any_member(&spec.name) && !crate::consent::asks_presence(&spec.name);
        if policy.requires(&spec.name, spec.kind) && !consent_free {
            engine.require_step_up(&spec.name);
        }
        // **Signed in means authenticated** (owner, 2026-10-22): a read asks
        // for no touch unless the deployment is strict
        // (`ORDO_PRESENCE_ON_READS`). The touch was also what kept a
        // capability token off a platform read such as `callersOf`, so a read
        // the app developer did not name in `ORDO_STEP_UP_EXEMPT_OPS` stays
        // undelegable by saying so directly.
        if spec.kind == OpKind::Read && !policy.exempts(&spec.name) {
            engine.forbid_delegation(&spec.name);
        }
        // Deployed with a pin like every other op. A platform op's manifest
        // is the hand-written spec, so the hash is over that document; it
        // touches no compiled entity, so the entity list is empty — the same
        // honest empty shape the deployment's own surfaces pin above.
        engine.deploy_op(
            &spec.name,
            ordo_identity::OpPin {
                manifest_hash: ordo_identity::principal::ManifestHash::of(
                    platform_ops::manifest(&spec).to_string().as_bytes(),
                ),
                unguarded: false,
                entities: Vec::new(),
            },
        );
    }
    // A profile per space the directory named. A
    // space with no `members` row gets none, and is right to: there is
    // nobody in it for a profile to grant anything to, and installing one
    // anyway would make a space that does not exist indistinguishable from
    // one that does to everything downstream of the engine.
    //
    // Each space gets the deployment's profile with its own lines laid over
    // it, and its own group links: what one tenant changed is read from that
    // tenant's rows and installed in that tenant alone.
    profile_ops::install_groups(&mut engine, tenant);
    let program: std::collections::BTreeSet<String> =
        specs.iter().map(|s| s.name.clone()).collect();
    for space in &spaces {
        let own = profile_ops::tenant_profile(&engine, &profile, space, tenant, &program);
        engine.set_profile(&SpaceId(space.clone()), own);
    }
    // What a delegated token may never call: every delete the app developer
    // did not open, and every platform write (`delegable`'s module doc).
    delegable::close(&mut engine, &specs, &platform_ops::specs(), delegating);
    Ok(engine)
}

/// **The compiler-to-door conversion**: what one op means, as the access
/// engine's plain data.
///
/// This is the one place `ordo_plan`'s typed compiler output becomes
/// `ordo_identity`'s pin. It exists here because those two crates never meet:
/// `ordo-identity` is a leaf with no sibling dependency, so a door decision
/// can never drag a compiler in, and `ordo-plan` knows nothing about grants.
/// `ordo-host` is the wiring crate and this is wiring.
///
/// Read off the **typed** producers ([`ordo_plan::artifact::op_scope`],
/// [`ordo_plan::artifact::op_row_scope`]) rather than re-parsed out of the
/// manifest JSON: a second reading of the same facts is how the manifest and
/// the pin come to disagree, and the pin is the half that decides
/// authorization.
///
/// The hash is over the whole catalog manifest — everything a consent page
/// renders, which is a superset of what the pin compares structurally. That
/// asymmetry is deliberate: the structural halves decide *narrowing vs
/// expansion*, and the hash notices that something else moved at all.
pub fn op_pin(f: &Fixture, op: &str) -> Option<ordo_identity::OpPin> {
    use ordo_identity::principal::ManifestHash;
    use ordo_identity::{EntityPin, GuardPin, OpPin};
    use ordo_plan::artifact::{op_row_scope, op_scope, GuardShape};

    let manifest_hash = ManifestHash::of(manifest(f, op)?.to_string().as_bytes());
    let fields = op_scope(&f.schema, &f.registry, op);
    let rows = op_row_scope(&f.schema, &f.registry, op);
    let shape = |g: &GuardShape| match g {
        GuardShape::Every => GuardPin::Every,
        GuardShape::Walk { walk, .. } => GuardPin::Walk(walk.clone()),
    };
    // Both producers walk the same entity list in the same order, so this
    // zips rather than joins — and if that ever stopped being true, the
    // `expect` below is where it would be noticed rather than where a pin
    // would quietly lose an entity's row bound.
    let entities = fields
        .iter()
        .map(|s| {
            let r = rows
                .entities
                .iter()
                .find(|r| r.entity == s.entity)
                .expect("op_scope and op_row_scope list the same entities");
            EntityPin {
                entity: s.entity.clone(),
                read: s.read.clone(),
                written: s.written.clone(),
                structure: s.structure,
                read_guard: shape(&r.read),
                write_guard: shape(&r.write),
                caller_restricted: r.caller_restricted,
            }
        })
        .collect();
    Some(OpPin {
        manifest_hash,
        unguarded: rows.unguarded,
        entities,
    })
}

/// The per-op pins a capability grant over `ops` must carry — exactly what
/// `deployment_engine` deployed, so a grant minted from this is alive until a
/// manifest actually changes.
pub fn grant_pins(
    f: &Fixture,
    ops: &std::collections::BTreeSet<String>,
) -> Option<std::collections::BTreeMap<String, ordo_identity::OpPin>> {
    let mut out = std::collections::BTreeMap::new();
    for op in ops {
        out.insert(op.clone(), op_pin(f, op)?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reported fix 25z. The pool's words and the door's are one string, held
    /// in two crates that cannot name each other; this is what keeps them
    /// equal. And the classifier every read and write goes through reads
    /// them as `server_busy`, not `internal`.
    #[test]
    fn a_saturated_pool_classifies_as_server_busy() {
        assert_eq!(ordo_exec::SATURATED_MESSAGE, ordo_api::SERVER_BUSY_MESSAGE);
        let e = classify(ordo_exec::PoolError::Saturated.to_string());
        assert_eq!(e.code, ErrorCode::ServerBusy);
        let e = classify(ordo_exec::ExecFail::Pool(ordo_exec::PoolError::Saturated).to_string());
        assert_eq!(e.code, ErrorCode::ServerBusy);
        // The typed path gives the same answer, word for word.
        let typed = exec_failure(ordo_exec::ExecFail::Pool(ordo_exec::PoolError::Saturated));
        assert_eq!((typed.code, &typed.message), (e.code, &e.message));
        let text = classify(ordo_exec::SATURATED_MESSAGE.to_string());
        assert_eq!(typed.message, text.message);
    }

    /// A tenant held closed while its data is brought to the current version
    /// of the app gets its own sentence (25i). It says what is happening and
    /// that the same request will land, and it names no database and no
    /// version, for §9.1 R-C4.4's reason: those are the deployment's
    /// topology, and they are in `/health/ready` and the log.
    #[test]
    fn a_tenant_being_brought_forward_is_told_so_without_topology() {
        let reason = "schema behind: this space's database is at `988b3664fe8b84b9` and this \
                      deployment serves `17a352d6a1c401c8`. It is held closed until it is \
                      brought to that version";
        let e = space_refusal(spaces::SpaceRefusal::SchemaBehind {
            space: "spc_x".into(),
            reason: reason.into(),
        });
        assert_eq!(e.code, ErrorCode::SpaceUnavailable);
        assert!(
            e.message
                .contains("brought to the current version of the app"),
            "{}",
            e.message
        );
        assert!(
            !e.message.contains("cannot reach"),
            "nothing is unreachable: {}",
            e.message
        );
        for leak in ["988b3664", "17a352d6", "database", "schema", "operator"] {
            assert!(!e.message.contains(leak), "`{leak}` in: {}", e.message);
        }

        // An unreachable database keeps its own sentence, names no address,
        // and no longer calls the deployment owner an operator.
        let e = space_refusal(spaces::SpaceRefusal::Unroutable {
            space: "spc_x".into(),
            reason: "connection refused at 10.0.0.7:5432".into(),
            class: ordo_exec::FaultClass::Broken,
        });
        assert!(e.message.contains("cannot reach"), "{}", e.message);
        assert!(!e.message.contains("10.0.0.7"), "{}", e.message);
        assert!(!e.message.contains("operator"), "{}", e.message);
    }
}
