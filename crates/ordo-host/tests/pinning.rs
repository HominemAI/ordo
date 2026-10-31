//! 8s — grant pinning, end to end, against real Postgres.
//!
//! # What this suite is for
//!
//! `definition-language.md` §8 rules that a pinned manifest carries the field
//! set **and the row-scope shape**, and calls the field-set-only form
//! specified by `security-logging-design.md` §5 and `api-engine-design.md`
//! §10 *"decidable and **wrong**"*. Its argument is a counterexample:
//!
//! > Delete `member == $caller` from an op body, or add `unguarded` to its
//! > header, and the op's **output field set is byte-identical**: same
//! > columns, same types, same order. A field-set comparison reads that as
//! > "not an expansion", classifies it as narrowing, and **applies it
//! > silently**. A grant a person consented to as *"this app may read my
//! > expenses"* becomes *"this app may read everyone's expenses"* with no
//! > re-consent prompt, no audit flag, and no visible change anywhere.
//!
//! `ordo-identity`'s own tests state the comparison rule over pins written by
//! hand. **This suite refuses to write any pin by hand.** Every shape here
//! comes out of the compiler, from real source text, and the escalation is
//! *executed against a real database first* — so the test does not merely
//! assert that a rule fires, it demonstrates that the edit the rule is about
//! really does hand one principal another principal's rows.
//!
//! Three claims, in order of how much they matter:
//!
//! 1. **The escalation is real.** Deleting `assignee == $caller` makes the op
//!    return other people's rows, on real data.
//! 2. **The losing rule cannot see it.** The field-grain scope manifest is
//!    *byte-identical* across that edit — asserted, not assumed.
//! 3. **The grant dies anyway**, because the row-scope shape moved.
//!
//! And the mirror image, because a rule that only ever denies is a rule
//! nobody can distinguish from the fail-closed combined hash it replaced:
//! **narrowing still applies silently.**
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::{BTreeMap, BTreeSet};

use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_identity::{
    AccessEngine, CapabilityGrant, Decision, DenyReason, GrantId, OpPin, PinDrift, PrincipalId,
    SpaceId, Timestamp, TokenClaims,
};

const NOW: Timestamp = Timestamp(1_000);
const EXPIRES: Timestamp = Timestamp(9_000);

/// Ana. A member of one project; `task_1` and `task_3` are hers.
const ANA: &str = "mem_1";
/// Bram. Assigned `task_3` **and** `task_5` — and `task_5` sits in a project
/// he is not a member of, so the entity guard hides it from him
/// (`guards-conformance` GC070). He is therefore the principal for whom
/// `unguarded` is visibly an escalation and not a no-op.
const BRAM: &str = "mem_2";

// ── the four programs ────────────────────────────────────────────────────
//
// One shipped fixture, one op, four spellings. Everything else — every
// entity, every guard, every other op — is byte-identical across all four,
// so nothing that moves can be attributed to anything but the edit.

/// The op as consented to. `assignee` is **projected as well as filtered on**,
/// which is what makes this the pure form of DL's counterexample: the filter
/// is the only thing the edit below removes, and it removes no column with it.
const CONSENTED: &str = "  tasks(assignee == $caller)[title, assignee, status]";

/// The edit. One condition deleted; the projection untouched.
const CALLER_DELETED: &str = "  tasks[title, assignee, status]";

/// The other half of DL's sentence: the same body, one flag on the header.
const UNGUARDED_HEADER: &str = "op myAssignedTasks unbounded unguarded =";

/// A genuine narrowing: one fewer column, the row bound untouched. Spelled
/// as a projection change rather than as the shipped fixture's own line,
/// because a "variant" that equals the original tests nothing.
const NARROWED: &str = "  tasks(assignee == $caller)[title, assignee]";

fn program(op_line: &str, header: Option<&str>) -> String {
    let src = FIXTURE.replace("  tasks(assignee == $caller)[title]", op_line);
    assert_ne!(src, FIXTURE, "the op-body edit found nothing to change");
    match header {
        None => src,
        Some(h) => {
            let out = src.replace("op myAssignedTasks unbounded =", h);
            assert_ne!(out, src, "the header edit found nothing to change");
            out
        }
    }
}

fn compile(src: &str) -> Fixture {
    Fixture::from_source(src, SEED).expect("the edited fixture compiles")
}

/// A database of this suite's own, seeded with the fixture. Named per test
/// (`ordo_pin_*`) so two tests never race on one schema, and never
/// `Pg::default()` — the shared conformance database is other suites'.
fn seeded(db: &str, f: &Fixture) -> Pg {
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    pg
}

// ── the pin, straight off the compiler ───────────────────────────────────

fn pins_for(f: &Fixture, ops: &[&str]) -> BTreeMap<String, OpPin> {
    let set: BTreeSet<String> = ops.iter().map(|s| s.to_string()).collect();
    ordo_host::grant_pins(f, &set).expect("every op is deployed")
}

/// A grant consented against `f`, and an engine serving `deployed`.
fn consented_against(
    f: &Fixture,
    deployed: &Fixture,
    pg: &Pg,
    ops: &[&str],
) -> (AccessEngine, TokenClaims) {
    let engine = ordo_host::deployment_engine(deployed, pg).expect("the deployment boots");
    let grant = CapabilityGrant::new(
        GrantId("gr_pin".into()),
        SpaceId(ordo_host::DEFAULT_SPACE.into()),
        pins_for(f, ops),
        EXPIRES,
    );
    let claims = TokenClaims {
        principal: PrincipalId(ANA.into()),
        capability: Some(grant),
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };
    (engine, claims)
}

/// The op's field-grain scope manifest, as JSON — the whole of what the
/// losing form of the rule compares.
fn scope(f: &Fixture) -> serde_json::Value {
    ordo_plan::artifact::scope_json(&ordo_plan::artifact::op_scope(
        &f.schema,
        &f.registry,
        "myAssignedTasks",
    ))
}

fn row_scope(f: &Fixture) -> serde_json::Value {
    ordo_plan::artifact::row_scope_json(&ordo_plan::artifact::op_row_scope(
        &f.schema,
        &f.registry,
        "myAssignedTasks",
    ))
}

fn titles(f: &Fixture, pg: &Pg, as_: &str) -> Vec<String> {
    f.run_case_as(pg, "myAssignedTasks", &[], Some(as_))
        .expect("the op runs")
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["title"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ═════════════════════════════════════════════════════════════════════════
// 1. The counterexample
// ═════════════════════════════════════════════════════════════════════════

/// **DL §8's counterexample, closed.**
///
/// The single most important assertion in this work: an op consented to as
/// *"my assigned tasks"* is edited into *"everyone's tasks"*, its field set
/// does not move by one byte, and the grant **dies** rather than silently
/// applying.
///
/// The test is in three acts and each one has to hold on its own. If act 1
/// stops holding, the edit is no longer an escalation and the test is about
/// nothing. If act 2 stops holding, the field-set comparison would have
/// caught it and the test no longer distinguishes the two rules. Act 3 is the
/// ruling.
#[test]
fn deleting_a_caller_restriction_kills_the_grant_though_no_column_moved() {
    let consented = compile(&program(CONSENTED, None));
    let widened = compile(&program(CALLER_DELETED, None));
    let pg = seeded("ordo_pin_caller_deleted", &consented);

    // ── act 1: the escalation is real, on real rows ──────────────────────
    //
    // Not "the compiler thinks this is wider" — the database hands back rows
    // that belong to somebody else. Both programs are run as the same
    // principal against the same seed.
    let before = titles(&consented, &pg, ANA);
    let after = titles(&widened, &pg, ANA);
    assert!(
        !before.is_empty(),
        "the consented op must return something, or act 1 proves nothing"
    );
    assert!(
        after.len() > before.len(),
        "deleting `assignee == $caller` must widen what comes back: {before:?} -> {after:?}"
    );
    let leaked: Vec<&String> = after.iter().filter(|t| !before.contains(t)).collect();
    assert!(
        !leaked.is_empty(),
        "the widened op returns rows the consented one did not: {leaked:?}"
    );

    // ── act 2: the losing rule is blind to it ────────────────────────────
    //
    // Byte-identical, at FIELD grain — which is the *stronger* of the two
    // field-set notions in play. §5 pinned the "ordered output field set",
    // which is `[title, assignee]` on both sides; 9l's manifest also counts
    // fields read in filters and guards, and even that does not move. There
    // is no field-shaped comparison that separates these two programs.
    assert_eq!(
        scope(&consented),
        scope(&widened),
        "the counterexample requires a byte-identical field manifest"
    );

    // ── act 3: the row-scope shape moved, and the grant dies ─────────────
    assert_ne!(row_scope(&consented), row_scope(&widened));
    let (engine, claims) = consented_against(&consented, &widened, &pg, &["myAssignedTasks"]);

    let drift = engine.drift(claims.capability.as_ref().unwrap());
    assert!(
        matches!(drift[0].1, PinDrift::Expanded { .. }),
        "a lost `$caller` restriction is an expansion: {:?}",
        drift[0].1
    );
    assert!(
        drift[0].1.note().contains("`$caller` restriction"),
        "the audit note must name what widened: {}",
        drift[0].1.note()
    );
    assert_eq!(
        engine.check(
            &claims,
            &SpaceId(ordo_host::DEFAULT_SPACE.into()),
            "myAssignedTasks",
            NOW
        ),
        Decision::Deny {
            op: "myAssignedTasks".into(),
            reason: DenyReason::ManifestChanged,
        },
        "the grant must be dead until re-consent"
    );
}

/// **The other half of DL's sentence**: *"or add `unguarded` to its header"*.
///
/// The op body is untouched — `assignee == $caller` is still there — and one
/// word on the header bypasses the entity guard that was doing the rest of
/// the work. This is the case a field-set comparison is *most* blind to,
/// because the guard-reached entities drop out of the manifest when the guard
/// stops being inlined, so the field set gets **smaller**: the losing rule
/// reads a guard bypass as a narrowing and applies it silently.
#[test]
fn adding_unguarded_kills_the_grant_though_the_field_set_only_shrank() {
    let consented = compile(&program(CONSENTED, None));
    let bypassed = compile(&program(CONSENTED, Some(UNGUARDED_HEADER)));
    let pg = seeded("ordo_pin_unguarded", &consented);

    // The escalation, on real rows: the guard was hiding tasks assigned to
    // Ana in projects she is not a member of (`guards-conformance` GC070).
    let before = titles(&consented, &pg, BRAM);
    let after = titles(&bypassed, &pg, BRAM);
    assert!(
        after.len() > before.len(),
        "`unguarded` must widen what comes back: {before:?} -> {after:?}"
    );

    // The field manifest SHRANK — a strict subset, which the losing rule
    // classifies as narrowing and applies with no prompt at all.
    let names = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|e| e["entity"].as_str().unwrap().to_string())
            .collect()
    };
    let (c, b) = (scope(&consented), scope(&bypassed));
    assert!(
        names(&b).len() < names(&c).len(),
        "the guard-reached entities leave the manifest: {:?} -> {:?}",
        names(&c),
        names(&b)
    );

    let (engine, claims) = consented_against(&consented, &bypassed, &pg, &["myAssignedTasks"]);
    let drift = engine.drift(claims.capability.as_ref().unwrap());
    assert!(
        matches!(drift[0].1, PinDrift::Expanded { .. }),
        "`unguarded` newly set is an expansion: {:?}",
        drift[0].1
    );
    assert!(
        drift[0].1.note().contains("unguarded"),
        "{}",
        drift[0].1.note()
    );
    assert_eq!(
        engine.check(
            &claims,
            &SpaceId(ordo_host::DEFAULT_SPACE.into()),
            "myAssignedTasks",
            NOW
        ),
        Decision::Deny {
            op: "myAssignedTasks".into(),
            reason: DenyReason::ManifestChanged,
        }
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 2. Narrowing, which must still apply silently
// ═════════════════════════════════════════════════════════════════════════

/// **The behaviour §5 wanted and the combined hash denied.**
///
/// This half is not a nicety. A combined hash kills a grant on *every*
/// manifest change, including one that gives the app strictly less — so an
/// author who deletes a column from a projection forces every consenting
/// person back through a consent screen. Consent screens people are made to
/// click through are consent screens they stop reading, and the security
/// property is the reading.
#[test]
fn narrowing_applies_silently_and_the_call_goes_through() {
    let consented = compile(&program(CONSENTED, None));
    let narrowed = compile(&program(NARROWED, None));
    let pg = seeded("ordo_pin_narrowed", &consented);

    // Strictly less: one fewer column, same rows.
    let field = |v: &serde_json::Value| v.as_array().unwrap().last().unwrap()["read"].clone();
    assert_eq!(
        field(&scope(&consented)),
        serde_json::json!(["assignee", "project", "status", "title"])
    );
    assert_eq!(
        field(&scope(&narrowed)),
        serde_json::json!(["assignee", "project", "title"]),
        "`status` is gone; `assignee` survives as the filter's read and \
         `title` as the sort key, which is exactly the point of stating the \
         manifest at FIELD grain rather than at output-column grain"
    );
    assert_eq!(
        row_scope(&consented),
        row_scope(&narrowed),
        "the row bound is untouched — this is a column narrowing and nothing else"
    );

    let (engine, claims) = consented_against(&consented, &narrowed, &pg, &["myAssignedTasks"]);
    let drift = engine.drift(claims.capability.as_ref().unwrap());
    assert!(
        matches!(drift[0].1, PinDrift::Narrowed { .. }),
        "less reach is narrowing: {:?}",
        drift[0].1
    );
    assert!(
        drift[0].1.note().contains("fewer fields"),
        "and it is audit-noted with what shrank: {}",
        drift[0].1.note()
    );
    assert!(
        engine
            .check(
                &claims,
                &SpaceId(ordo_host::DEFAULT_SPACE.into()),
                "myAssignedTasks",
                NOW
            )
            .is_allow(),
        "a person who consented to more must not be asked again for less"
    );
}

/// **The per-op half of the ruling**, which everyone agreed on and which the
/// combined hash made unimplementable.
///
/// A grant lists two ops. An op the caller is not even calling narrows. Under
/// one combined hash over all listed ops, that kills the grant — *"narrowing
/// changes the hash too"* — and the caller is refused for a change that took
/// authority away from an op they were not using.
#[test]
fn an_unrelated_ops_narrowing_does_not_kill_the_grant() {
    let consented = compile(&program(CONSENTED, None));
    let narrowed = compile(&program(NARROWED, None));
    let pg = seeded("ordo_pin_unrelated", &consented);

    // The grant covers `myAssignedTasks` and `openTasks`; only the former
    // changes, and the call is to the latter.
    let ops = ["myAssignedTasks", "openTasks"];
    let (engine, claims) = consented_against(&consented, &narrowed, &pg, &ops);
    assert_eq!(
        pins_for(&consented, &["openTasks"])["openTasks"],
        pins_for(&narrowed, &["openTasks"])["openTasks"],
        "the op being called did not change at all"
    );
    assert!(
        engine
            .check(
                &claims,
                &SpaceId(ordo_host::DEFAULT_SPACE.into()),
                "openTasks",
                NOW
            )
            .is_allow(),
        "an unrelated op's narrowing must not invalidate a consent"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 3. The shape has to be pinnable at all
// ═════════════════════════════════════════════════════════════════════════

/// **A pin is only a pin if two deployments of the same program agree on
/// it.** Compiled twice, from the same text, through two independent
/// `Fixture`s: same hash, same field set, same row-scope shape, byte for
/// byte — for **every** registered op, not a sampled one.
///
/// Stated as its own test because the alternative is a comparison that fires
/// on nothing an author did. Aliases are minted from a per-bind counter and
/// guard walks are re-rendered on every call, so "it happened to match once"
/// is not the same claim.
#[test]
fn the_pinned_shape_is_identical_across_two_compilations() {
    let a = compile(&program(CONSENTED, None));
    let b = compile(&program(CONSENTED, None));
    let names: BTreeSet<String> = a.registry.names().cloned().collect();
    assert!(names.len() > 20, "the fixture registers a real surface");
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        pins_for(&a, &refs),
        pins_for(&b, &refs),
        "the same program must pin the same shape on every machine and every run"
    );
}

/// The row-scope shape is not decorative: for the fixture's guarded ops it
/// actually carries a walk, and for its `$caller`-bodied op it actually
/// carries the restriction. A shape that came out empty would pass every
/// comparison test above by comparing nothing.
#[test]
fn the_row_scope_shape_is_populated_for_the_ops_it_is_about() {
    let f = compile(&program(CONSENTED, None));
    let rs = row_scope(&f);
    let tasks = rs["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entity"] == "tasks")
        .expect("`tasks` is in the shape");
    assert_eq!(
        tasks["read"]["walk"], "project.projectMembers(member == $caller)",
        "the effective read guard is pinned by its walk, not by a flag"
    );
    assert_eq!(tasks["caller_restricted"], true);
    assert_eq!(rs["unguarded"], false);
    // Routines do not exist (`corpus-rulings.md`: `routine` is `[lean]`, and
    // nothing drains the job queue), so DL's trigger-class clause has no
    // producer. The key is present and null rather than absent, so the day it
    // gains a value that is a value change and not a schema change.
    assert_eq!(rs["trigger"], serde_json::Value::Null);
}
