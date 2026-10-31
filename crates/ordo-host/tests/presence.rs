//! **Where a platform op requires presence is the app developer's setting**
//! (owner, 2026-09-29: *"What is enforced is app design ... not our
//! design"*), and the default is that every change does. **A read asks for
//! none** (owner, 2026-10-22: *"If you are signed in = that passkey was your
//! credentials - you are authenticated."*).
//!
//! What is proved:
//!
//! - every platform op is classified read or change by its own `OpKind`,
//!   and the reads are the list pinned here;
//! - with nothing set, a read answers a signed-in person with no touch and
//!   every change demands a fresh passkey assertion;
//! - strict (`ORDO_PRESENCE_ON_READS=on`) asks on reads too;
//! - a read freed this way is still not delegable;
//! - the setting names ops exactly or by a `.*` prefix, and only the named
//!   ones stop asking;
//! - an exempt op is still refused to a caller the role gate does not list,
//!   so removing presence removes only presence;
//! - the ops the owner named (apply, create, import, export, delete) can each
//!   be exempted, and a neighbour of an exempt op still asks.

use std::sync::Arc;

use ordo_api::{AccessChecker, AccessDecision, OpKind};
use ordo_conformance::Fixture;
use ordo_host::console_keys::NEVER_EXEMPT;
use ordo_host::presence::PresencePolicy;
use ordo_host::IdentityAccess;
use ordo_identity::principal::{PrincipalId, Timestamp};
use ordo_identity::{TokenKey, TokenSpec};

const PROGRAM: &str = "\
entity notes (key: nte) {
  title: text
}

op listNotes =
  notes[id, title]
";
const SECRET: &[u8] = b"presence-secret";
const NOW: u64 = 1_000;
const SPACE: &str = "spc_a";

fn names() -> Vec<String> {
    ordo_host::platform_ops::specs()
        .into_iter()
        .map(|s| s.name)
        .collect()
}

fn row(id: &str, role: &str) -> Vec<Option<String>> {
    vec![
        Some(SPACE.to_string()),
        Some(id.to_string()),
        Some(role.to_string()),
        Some("active".to_string()),
        Some(id.to_string()),
    ]
}

fn token(principal: &str, present: bool) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(NOW + 3_600),
            client: None,
            user_presence_at: present.then_some(Timestamp(NOW)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

fn door(policy: &PresencePolicy) -> IdentityAccess {
    let f = Fixture::from_source(PROGRAM, "").expect("the program compiles");
    let engine = ordo_host::engine_from_members_under(
        &f,
        &[row("usr_owner", "owner"), row("usr_editor", "editor")],
        policy,
    )
    .expect("the engine builds");
    IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    )
}

use ordo_host::platform_ops::person_scoped;

/// Ops that never ask for presence whatever the policy: a person's own
/// list of tenants, and the consent ops other than the approval (11-4b).
fn presence_free(name: &str) -> bool {
    person_scoped(name)
        || (ordo_host::consent::any_member(name) && !ordo_host::consent::asks_presence(name))
}

fn allowed(d: &IdentityAccess, who: &str, present: bool, op: &str) -> bool {
    matches!(
        d.check(&token(who, present), SPACE, op),
        AccessDecision::Allow { .. }
    )
}

fn kind(op: &str) -> OpKind {
    ordo_host::platform_ops::specs()
        .into_iter()
        .find(|s| s.name == op)
        .map(|s| s.kind)
        .expect("a platform op")
}

fn reads() -> Vec<String> {
    names()
        .into_iter()
        .filter(|n| kind(n) == OpKind::Read)
        .collect()
}

/// **Every platform op is classified, by its own declaration.** Read or
/// change is the op's `OpKind`, the field that also makes it a `GET`. This
/// pins the list of reads, so an op that becomes a read, or a new read,
/// shows up here and is looked at: from then on it answers with no touch.
#[test]
fn every_platform_op_is_classified_and_the_reads_are_these() {
    let mut got = reads();
    got.sort();
    let mut want: Vec<String> = READS.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(got, want, "the platform reads changed; review each new one");
    // Every other op is a change, and asks.
    for op in names() {
        assert!(
            READS.contains(&op.as_str()) || matches!(kind(&op), OpKind::Write | OpKind::Delete),
            "{op} is neither a read nor a change"
        );
    }
}

/// The platform ops that only read. Each answers a signed-in person with no
/// touch unless the deployment is strict.
const READS: &[&str] = &[
    "std.delegation.mine",
    "std.delegation.review",
    "std.deployment.observedCost",
    "std.deployment.consoleKeys",
    // The app secrets' names and hosts, never a value.
    "std.deployment.secrets",
    // Function runs and one run's log: the deployment owner's to read.
    "std.deployment.functionRuns",
    "std.deployment.functionRun",
    "std.deployment.versions",
    // The open recoveries and successions, with objections: the deployment
    // owner's to read, home space (owner succession, 2026-10-24).
    "std.identity.openCases",
    "std.deployment.tenants",
    "std.deployment.settings",
    "std.deployment.quotas",
    "std.deployment.securityEvents",
    "std.person.tenants",
    "std.space.admission",
    "std.space.attestation",
    "std.space.attestationDeadLetters",
    "std.space.callersOf",
    "std.space.chainProof",
    "std.space.connections",
    "std.space.groups",
    // G5, the People reads (console step 8).
    "std.space.members",
    "std.space.grants",
    "std.space.groupMembers",
    "std.space.callableBy",
    "std.space.invitations",
    "std.space.links",
    "std.space.origins",
    "std.space.profile",
    "std.space.sessionPolicy",
    "std.space.shares",
];

/// **A read answers a signed-in person with no touch.**
#[test]
fn by_default_a_read_needs_no_presence_and_a_change_does() {
    let d = door(&PresencePolicy::standard());
    for op in names() {
        let without = allowed(&d, "usr_owner", false, &op);
        let with = allowed(&d, "usr_owner", true, &op);
        if kind(&op) == OpKind::Read || presence_free(&op) {
            assert_eq!(without, with, "{op}: a read asked for a touch");
        } else {
            assert!(!without, "{op} changed something with no presence");
        }
    }
    for op in ["std.space.sessionPolicy", "std.space.callersOf"] {
        assert!(allowed(&d, "usr_owner", false, op), "{op}");
    }
    // A change still asks.
    assert!(!allowed(
        &d,
        "usr_owner",
        false,
        "std.space.setSessionPolicy"
    ));
    assert!(allowed(&d, "usr_owner", true, "std.space.setSessionPolicy"));
    assert!(!allowed(&d, "usr_owner", false, "std.space.admit"));
    // The role gate is untouched: an editor still cannot read callersOf.
    assert!(!allowed(&d, "usr_editor", false, "std.space.callersOf"));
}

/// **A read freed by the ruling is still not delegable**, as it was while
/// the touch kept capability tokens off it. A read the app developer names
/// in `ORDO_STEP_UP_EXEMPT_OPS` is, as before.
#[test]
fn a_read_freed_by_default_is_not_delegable() {
    let f = Fixture::from_source(PROGRAM, "").expect("the program compiles");
    let rows = [row("usr_owner", "owner")];
    let engine = ordo_host::engine_from_members_under(&f, &rows, &PresencePolicy::standard())
        .expect("the engine builds");
    // A person's own list of tenants asked for no touch before the ruling
    // and stays as it was.
    for op in reads().into_iter().filter(|n| !person_scoped(n)) {
        assert!(!engine.delegable(&op), "{op} became delegable");
    }
    let named = PresencePolicy::parse("std.space.callersOf", &names()).expect("ok");
    let engine =
        ordo_host::engine_from_members_under(&f, &rows, &named).expect("the engine builds");
    assert!(engine.delegable("std.space.callersOf"));
}

/// **Strict restores presence on reads** (`ORDO_PRESENCE_ON_READS=on`):
/// every platform op asks, as before the ruling.
#[test]
fn strict_asks_presence_on_every_platform_op() {
    let d = door(&PresencePolicy::standard().with_reads(true));
    assert!(names().len() >= 15, "the list this test walks is real");
    // A person-scoped op (`std.person.tenants`) reads the caller's own
    // memberships and never asks for presence; every other op does.
    for op in names().into_iter().filter(|n| !presence_free(n)) {
        assert!(
            !allowed(&d, "usr_owner", false, &op),
            "{op} answered an owner with no presence"
        );
    }
    assert!(allowed(
        &d,
        "usr_owner",
        false,
        ordo_host::platform_ops::MY_TENANTS
    ));
    // Reading, listing and revoking a delegation ask for none; approving one
    // does (11-4b).
    assert!(allowed(&d, "usr_owner", false, ordo_host::consent::MINE));
    assert!(allowed(&d, "usr_owner", false, ordo_host::consent::REVOKE));
    assert!(!allowed(
        &d,
        "usr_owner",
        false,
        ordo_host::consent::APPROVE
    ));
    assert!(allowed(&d, "usr_owner", true, ordo_host::consent::APPROVE));
    // The control: with presence the owner gets the ones the owner may call.
    assert!(allowed(&d, "usr_owner", true, "std.space.callersOf"));
    assert!(allowed(&d, "usr_owner", true, "std.space.admit"));
}

#[test]
fn a_named_op_stops_asking_and_its_neighbour_does_not() {
    let policy = PresencePolicy::parse("std.space.admit, std.space.callersOf", &names())
        .expect("both name platform ops");
    assert!(policy.exempts("std.space.admit"));
    assert!(policy.exempts("std.space.callersOf"));
    assert!(!policy.exempts("std.space.remove"));
    let d = door(&policy);
    assert!(allowed(&d, "usr_owner", false, "std.space.admit"));
    assert!(allowed(&d, "usr_owner", false, "std.space.callersOf"));
    assert!(
        !allowed(&d, "usr_owner", false, "std.space.remove"),
        "an op nobody named still asks"
    );
}

#[test]
fn a_prefix_names_every_op_under_it() {
    let policy = PresencePolicy::parse("std.space.*", &names()).expect("a prefix");
    assert!(policy.exempts("std.space.admit"));
    assert!(policy.exempts("std.space.delete"));
    assert!(
        !policy.exempts("std.deployment.apply"),
        "`std.space.*` is not `std.*`"
    );
    // A prefix is a whole word: it does not reach `std.spacex`.
    assert!(!policy.exempts("std.spacex.admit"));
    let all = PresencePolicy::parse("std.*", &names())
        .expect("all of them")
        .with_reads(true);
    assert!(names()
        .iter()
        .filter(|n| !NEVER_EXEMPT.contains(&n.as_str()))
        .all(|n| !all.requires(n, kind(n))));
}

/// Security review, step 3 (finding 4): who holds a console key is decided
/// with the owner's own touch, whatever `ORDO_STEP_UP_EXEMPT_OPS` says. A
/// prefix leaves the two ops out; naming one refuses the boot.
#[test]
fn the_console_key_ops_can_never_be_exempted() {
    for list in ["std.deployment.*", "std.*"] {
        let policy = PresencePolicy::parse(list, &names()).expect(list);
        assert!(policy.exempts("std.deployment.apply"), "{list}");
        for op in NEVER_EXEMPT {
            assert!(!policy.exempts(op), "{list} exempted {op}");
            assert!(policy.requires(op, OpKind::Write), "{list}: {op}");
        }
        let d = door(&policy);
        for op in NEVER_EXEMPT {
            assert!(
                !allowed(&d, "usr_owner", false, op),
                "{list}: the engine still asks for a touch on {op}"
            );
            assert!(allowed(&d, "usr_owner", true, op), "{list}: {op}");
        }
    }
    for op in NEVER_EXEMPT {
        let err = PresencePolicy::parse(op, &names()).expect_err(op);
        assert!(err.contains(op), "{err}");
        assert!(err.contains("ORDO_STEP_UP_EXEMPT_OPS"), "{err}");
    }
}

/// The ops the owner named can each be exempted, one at a time, and the
/// others keep asking.
#[test]
fn the_owner_named_ops_can_each_be_exempted() {
    let named = [
        "std.deployment.apply",
        "std.space.create",
        "std.space.import",
        "std.space.export",
        "std.space.delete",
    ];
    for op in named {
        // Strict, so reads ask too and every op but the named one does.
        let policy = PresencePolicy::parse(op, &names())
            .expect(op)
            .with_reads(true);
        for other in named {
            assert_eq!(
                policy.requires(other, kind(other)),
                other != op,
                "{op} / {other}"
            );
        }
        let d = door(&policy);
        // The engine built under it asks for presence on every other op.
        for other in names().into_iter().filter(|n| !presence_free(n)) {
            let asks = !matches!(
                d.check(&token("usr_owner", false), SPACE, &other),
                AccessDecision::Allow { .. }
            );
            assert_eq!(asks, other != op, "{op}: engine on {other}");
        }
    }
}

/// Removing presence removes presence and nothing else.
#[test]
fn an_exempt_op_is_still_refused_to_a_caller_the_role_gate_does_not_list() {
    let policy = PresencePolicy::parse("std.space.admit,std.space.export", &names()).expect("ok");
    let d = door(&policy);
    assert!(allowed(&d, "usr_owner", false, "std.space.admit"));
    for op in ["std.space.admit", "std.space.export"] {
        assert!(
            !allowed(&d, "usr_editor", true, op),
            "an editor is not in the role list for {op}, with or without presence"
        );
    }
}

#[test]
fn a_name_that_is_no_platform_op_is_refused_and_the_real_ones_are_listed() {
    let err = PresencePolicy::parse("std.space.admit,std.space.nothing", &names())
        .expect_err("a misspelt op must not be silently ignored");
    assert!(err.contains("std.space.nothing"), "{err}");
    assert!(err.contains("std.space.callersOf"), "{err}");
    assert!(PresencePolicy::parse("nope.*", &names()).is_err());
    assert!(PresencePolicy::parse("", &names())
        .expect("empty is the default")
        .exempt()
        .is_empty());
}
