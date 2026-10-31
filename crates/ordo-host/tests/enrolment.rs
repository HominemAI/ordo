//! **Enrolment E1: the door resolves a person and a tenant to a member.**
//!
//! `plan-enrolment.md` §0 found that a sign-in mints a token for the
//! *principal* the directory resolved, while the door keyed every membership
//! by `members.id`. A principal `usr_abc` was a stranger in every tenant. It
//! worked in tests and in `ordo dev` only because both hand themselves a
//! token whose subject is the member id.
//!
//! These tests use rows whose `principal` and `id` differ, which is the case
//! the earlier tests never built.

use std::sync::Arc;

use ordo_api::{AccessChecker, AccessDecision};
use ordo_conformance::Fixture;
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
const SECRET: &[u8] = b"enrolment-secret";
const NOW: u64 = 1_000;

fn row(space: &str, id: &str, role: &str, principal: &str) -> Vec<Option<String>> {
    vec![
        Some(space.to_string()),
        Some(id.to_string()),
        Some(role.to_string()),
        Some("active".to_string()),
        Some(principal.to_string()),
    ]
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

fn door(rows: &[Vec<Option<String>>]) -> IdentityAccess {
    let f = Fixture::from_source(PROGRAM, "").expect("the program compiles");
    let engine = ordo_host::engine_from_members(&f, rows).expect("the engine builds");
    IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    )
}

fn allowed_as(d: &IdentityAccess, who: &str, space: &str) -> Option<String> {
    match d.check(&token(who), space, "listNotes") {
        AccessDecision::Allow { principal, .. } => Some(principal),
        AccessDecision::Deny => None,
    }
}

/// One principal with memberships in two tenants is served in both with a
/// different `$caller` in each, and refused in a third.
#[test]
fn one_principal_in_two_tenants_is_a_different_member_in_each_and_a_stranger_in_a_third() {
    let door = door(&[
        row("spc_a", "mem_a_ana", "editor", "usr_ana"),
        row("spc_b", "mem_b_ana", "viewer", "usr_ana"),
        row("spc_a", "mem_a_bo", "viewer", "usr_bo"),
        row("spc_c", "mem_c_bo", "owner", "usr_bo"),
    ]);

    assert_eq!(
        allowed_as(&door, "usr_ana", "spc_a").as_deref(),
        Some("mem_a_ana"),
        "served in the first tenant, as that tenant's member"
    );
    assert_eq!(
        allowed_as(&door, "usr_ana", "spc_b").as_deref(),
        Some("mem_b_ana"),
        "served in the second tenant, as a different member"
    );
    assert_eq!(
        allowed_as(&door, "usr_ana", "spc_c"),
        None,
        "refused in a tenant she is not in"
    );
}

/// The member id is not a way in. Only the person the row names is, so a
/// token whose subject is somebody's member id in a tenant proves nothing
/// about that person once the row names a principal.
#[test]
fn a_member_id_is_not_a_subject_when_the_row_names_a_principal() {
    let door = door(&[row("spc_a", "mem_a_ana", "editor", "usr_ana")]);
    assert_eq!(allowed_as(&door, "mem_a_ana", "spc_a"), None);
}

/// A row with no principal, and the old four-column shape, are keyed by
/// their id: the member id standing in as the subject, which is what the
/// tests and `ordo dev` mint their tokens with.
#[test]
fn a_row_with_no_principal_is_keyed_by_its_member_id() {
    let mut no_principal = row("spc_a", "mem_x", "editor", "");
    no_principal[4] = None;
    let empty = row("spc_a", "mem_y", "editor", "");
    let four = row("spc_a", "mem_z", "editor", "")[..4].to_vec();
    let door = door(&[no_principal, empty, four]);
    for id in ["mem_x", "mem_y", "mem_z"] {
        assert_eq!(allowed_as(&door, id, "spc_a").as_deref(), Some(id), "{id}");
    }
}

/// The statement asks for the column, at the end, because every reader
/// indexes by position.
#[test]
fn the_directory_read_selects_principal_last() {
    let cols: Vec<&str> = ordo_host::MEMBERS_SQL
        .split("FROM")
        .next()
        .unwrap()
        .trim_start_matches("SELECT")
        .split(',')
        .map(str::trim)
        .collect();
    assert_eq!(
        cols,
        [
            "\"$space\"",
            "\"id\"",
            "\"role\"",
            "\"status\"",
            "\"principal\"",
            // 27ah: a group row is skipped as a membership.
            "\"kind\""
        ]
    );
}
