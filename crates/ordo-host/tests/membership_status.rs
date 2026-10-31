//! **A member's `status` reaches the door.**
//!
//! `definition-language.md` gives every member a status of `active` or
//! `suspended` and says *"suspension is a command, not a delete"*. The access
//! engine has implemented suspension since it was written, and the door's
//! directory read selected three columns of four, so every member loaded as
//! active and `AccessEngine::suspend` had no caller outside its own tests.
//! The 2026-09-28 security review set a member to `suspended` in the
//! database of a running deployment and they kept full access
//! (`security-review-6.md` S6-002).
//!
//! The owner's requirement for the fix is that nobody restarts anything:
//! *"We shouldn't be restarting deployments because someone got or lost
//! access."* So there are two claims here. The engine built from the rows
//! honours the status, and the statement that reads the rows asks for it.
//! The second is asserted against the constant itself, because the defect
//! was the constant.

use ordo_conformance::Fixture;
use ordo_identity::principal::MembershipStatus;
use ordo_identity::principal::{PrincipalId, SpaceId};

const PROGRAM: &str = "\
entity notes (key: nte) {
  title: text
}

op listNotes =
  notes[id, title]
";

fn row(space: &str, id: &str, role: &str, status: Option<&str>) -> Vec<Option<String>> {
    vec![
        Some(space.to_string()),
        Some(id.to_string()),
        Some(role.to_string()),
        status.map(str::to_string),
    ]
}

#[test]
fn a_suspended_member_is_suspended_at_the_door() {
    let f = Fixture::from_source(PROGRAM, "").expect("the program compiles");
    let rows = vec![
        row("spc_a", "mem_active", "editor", Some("active")),
        row("spc_a", "mem_gone", "editor", Some("suspended")),
    ];
    let engine = ordo_host::engine_from_members(&f, &rows).expect("the engine builds");
    let space = SpaceId("spc_a".into());

    assert_eq!(
        engine.membership_status(&space, &PrincipalId("mem_active".into())),
        Some(MembershipStatus::Active)
    );
    assert_eq!(
        engine.membership_status(&space, &PrincipalId("mem_gone".into())),
        Some(MembershipStatus::Suspended),
        "a row that says suspended must load as suspended"
    );
}

/// Suspension is per space. The same person suspended in one tenant is an
/// ordinary member of another, because a membership is a fact about a
/// person IN a space and the row is keyed that way.
#[test]
fn suspension_in_one_space_is_not_suspension_in_another() {
    let f = Fixture::from_source(PROGRAM, "").expect("the program compiles");
    let rows = vec![
        row("spc_a", "mem_x", "editor", Some("suspended")),
        row("spc_b", "mem_x", "editor", Some("active")),
    ];
    let engine = ordo_host::engine_from_members(&f, &rows).expect("the engine builds");
    let who = PrincipalId("mem_x".into());
    assert_eq!(
        engine.membership_status(&SpaceId("spc_a".into()), &who),
        Some(MembershipStatus::Suspended)
    );
    assert_eq!(
        engine.membership_status(&SpaceId("spc_b".into()), &who),
        Some(MembershipStatus::Active)
    );
}

/// A null status, a status this build has never heard of, and a row from a
/// caller still handing over three columns all load as **active** and none
/// of them panics the door. Refusing a whole space's members over an
/// unrecognised word would be an outage caused by a typo.
#[test]
fn an_unknown_or_absent_status_is_active_and_never_a_panic() {
    let f = Fixture::from_source(PROGRAM, "").expect("the program compiles");
    let three_columns = vec![
        Some("spc_a".to_string()),
        Some("mem_old".to_string()),
        Some("editor".to_string()),
    ];
    let rows = vec![
        row("spc_a", "mem_null", "editor", None),
        row("spc_a", "mem_odd", "editor", Some("on-holiday")),
        three_columns,
    ];
    let engine = ordo_host::engine_from_members(&f, &rows).expect("the engine builds");
    for id in ["mem_null", "mem_odd", "mem_old"] {
        assert_eq!(
            engine.membership_status(&SpaceId("spc_a".into()), &PrincipalId(id.into())),
            Some(MembershipStatus::Active),
            "{id}"
        );
    }
}

/// **The statement asks for the column.** This is the assertion that would
/// have caught the defect: the engine was always able to suspend, and the
/// query never told it to.
#[test]
fn the_directory_read_selects_status() {
    assert!(
        ordo_host::MEMBERS_SQL.contains("\"status\""),
        "the door's member read must select `status`: {}",
        ordo_host::MEMBERS_SQL
    );
    // And at the end, because every reader indexes by position.
    let cols: Vec<&str> = ordo_host::MEMBERS_SQL
        .split("FROM")
        .next()
        .unwrap()
        .trim_start_matches("SELECT")
        .split(',')
        .map(str::trim)
        .collect();
    assert_eq!(
        cols[..4],
        ["\"$space\"", "\"id\"", "\"role\"", "\"status\""],
        "the first four columns keep their positions; `principal` is appended (E1)"
    );
}
