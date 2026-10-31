//! Bootstrap rule 6: a space owner administers any acl row in their space.
//!
//! DL §3, `[settled]`:
//!
//! > A space owner may grant, revoke, and change levels on every acl-derived
//! > link entity in the space **without holding an access row on anything**.
//! > … the owner capability is evaluated **at the door, once per call, before
//! > planning** — not as an extra disjunct in each row's guard. When it
//! > applies, the derived link entity's guards are not consulted for that
//! > call.
//!
//! None of it existed. Found by the settled-versus-code audit, which ran it:
//! the space owner got `not_found` on grant and revoke, and
//! `setBoardsAccessLevel` answered **`updated: 0` inside a success
//! envelope** — a silent no-op on an authorization change, which is the
//! worst available shape for one.
//!
//! **This is the permanent lockout the rule exists to break**, and it matters
//! beyond itself: 10b ruled that a guest- or routine-created row landing
//! ungranted is safe *because rule 6 makes the space owner the universal
//! repair path*. That path did not exist, so those rows were administrable by
//! nobody.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::{Fixture, Pg, WriteLog};

/// `mem_o` is the space owner in the guards fixture; `mem_2` is an ordinary
/// editor holding no access row.
const OWNER: Option<&str> = Some("mem_o");
const EDITOR: Option<&str> = Some("mem_2");

fn setup(db: &str) -> (Fixture, Pg) {
    let f = Fixture::load().expect("fixture");
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    (f, pg)
}

/// Read the stored level straight from the table.
///
/// Deliberately not through a guarded read as the owner: rule 6 confers
/// **grant, revoke and level change**, and says nothing about reading. An
/// owner holding no access row still cannot *see* the rows they administer,
/// which is worth knowing and is a separate question from this one. The
/// assertion here is about what landed in the database.
fn level_of(pg: &Pg, board: &str, member: &str) -> Option<String> {
    let rows = pg
        .query(&format!(
            "SELECT \"level\" FROM \"boardsAccess\" \
             WHERE \"board\" = '{board}' AND \"member\" = '{member}'"
        ))
        .expect("query");
    rows.first().and_then(|r| r.first().cloned().flatten())
}

/// The three verbs, all of which the owner could not perform.
#[test]
fn the_space_owner_can_grant_revoke_and_change_levels() {
    let (f, pg) = setup("ordo_owner_admin");
    let mut log = WriteLog::new();

    // Grant — answered `not_found` before, because the owner holds no access
    // row on `board_1` and the derived guard asks for one.
    f.run_write_as(
        &pg,
        &mut log,
        r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")"#,
        OWNER,
    )
    .expect("an owner may grant without holding an access row");
    assert_eq!(level_of(&pg, "board_1", "mem_4").as_deref(), Some("read"));

    // Level change — the silent one. It answered `updated: 0` inside a
    // SUCCESS envelope, so a caller was told the authorization change landed
    // when it had not.
    let out = f
        .run_write_as(
            &pg,
            &mut log,
            r#"setBoardsAccessLevel($board: "board_1", $member: "mem_4", $level: "admin")"#,
            OWNER,
        )
        .expect("an owner may change a level");
    assert_ne!(
        out[0]["updated"], 0,
        "a success envelope reporting zero rows is the shape this closes: {out}"
    );
    assert_eq!(
        level_of(&pg, "board_1", "mem_4").as_deref(),
        Some("admin"),
        "and the change is in the row, not just in the envelope"
    );

    // Revoke.
    f.run_write_as(
        &pg,
        &mut log,
        r#"revokeBoardsAccess($board: "board_1", $member: "mem_4")"#,
        OWNER,
    )
    .expect("an owner may revoke");
    assert_eq!(level_of(&pg, "board_1", "mem_4"), None);
}

/// The capability is **scoped**, and this is the half that keeps it from
/// being a bypass. Rule 6 is about acl administration in one space — not
/// about ordinary rows, which an owner meets as any other caller does.
#[test]
fn the_owner_capability_does_not_reach_ordinary_entities() {
    let (f, pg) = setup("ordo_owner_admin_scope");
    let mut log = WriteLog::new();

    // `mem_o` owns the space and is not a member of `proj_1`, so the
    // ordinary task guard refuses — rule 6 confers nothing here.
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"renameTask($task: "task_1", $title: "owned")"#,
            OWNER,
        )
        .expect_err("rule 6 is acl administration, not a master key");
    assert!(
        e.contains("not_found") || e.contains("access_guard"),
        "the owner meets the ordinary guard like anyone else: {e}"
    );
}

/// And it is the OWNER's capability, not everyone's. An editor with no
/// access row still cannot administer acl rows — if this passed, the door
/// step would be granting the exemption to the wrong callers.
#[test]
fn a_non_owner_still_cannot_administer_acl_rows() {
    let (f, pg) = setup("ordo_owner_admin_editor");
    let mut log = WriteLog::new();

    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")"#,
            EDITOR,
        )
        .expect_err("an editor holding no access row may not grant");
    assert!(e.contains("not_found") || e.contains("access_guard"), "{e}");
}

/// The mechanism is the one the rule names. An extra `or caller is owner`
/// disjunct inside each guard is what rule 6 explicitly forbids, because it
/// makes ownership a row-level fact every guard must re-derive — the per-row
/// reading that reintroduces the deadlock. So the guard text must be
/// unchanged, and the exemption must live in the call.
#[test]
fn the_derived_guard_is_not_rewritten() {
    let f = Fixture::load().expect("fixture");
    let rendered = f
        .schema
        .entity("boardsAccess")
        .expect("link entity")
        .guards
        .as_ref()
        .expect("guarded")
        .render(true);
    assert!(
        !rendered.contains("owner") && !rendered.contains("role"),
        "the owner capability must not appear in the guard: {rendered}"
    );
}

/// **The owner cannot read the rows they administer, and that is the ruling
/// rather than a gap.**
///
/// DL §3 `[settled — resolves Z-004]`: *"No acl row is private from the space
/// owner, and that is the definition of an owner… An owner who can administer
/// acl rows can grant themselves any level on any row, so nothing inside their
/// own space is cryptographically or structurally hidden from them. This is a
/// disclosed property, not a hole."* — followed by the obligation that decides
/// this test: *"every such reach is an ordinary attributed command in the
/// space's chain (**the owner cannot look without leaving a record**)."*
///
/// Reads append nothing to the chain; only writes do. So a read exemption
/// would be precisely the reach that leaves no record. The owner's route is
/// to grant themselves a level — an attributed command, visible to every
/// member — and then read as an ordinary holder. The friction *is* the audit
/// trail.
#[test]
fn the_owner_reaches_by_granting_and_the_grant_is_the_record() {
    let (f, pg) = setup("ordo_owner_reach");
    let mut log = WriteLog::new();

    // Blind first: administration confers no sight.
    let before = f
        .run_case_as(&pg, r#"boardsAccess[level]"#, &[], OWNER)
        .expect("the read itself is permitted; it simply sees nothing");
    assert_eq!(
        before.as_array().map(|a| a.len()),
        Some(0),
        "rule 6 confers administration, not sight: {before}"
    );

    // The route the ruling describes — and it is a write, so it lands in the
    // chain. That is the whole point: the owner cannot look without saying so.
    let groups_before = log.len();
    f.run_write_as(
        &pg,
        &mut log,
        r#"grantBoardsAccess($board: "board_1", $member: "mem_o", $level: "admin")"#,
        OWNER,
    )
    .expect("an owner may grant themselves any level — that is what owning is");
    assert_eq!(
        log.len(),
        groups_before + 1,
        "the reach left a record; a read exemption would not have"
    );

    // …and now they see it, as an ordinary holder of a level.
    let after = f
        .run_case_as(&pg, r#"boardsAccess[level]"#, &[], OWNER)
        .expect("reads");
    assert!(
        !after.as_array().expect("rows").is_empty(),
        "having granted, the owner reads through the ordinary guard: {after}"
    );
}

// ─────────── rule 6's second half: the flag, end to end (16c) ───────────
//
// *"logged with the owner as actor and **flagged as owner
// administration**"*. The actor half shipped with the mechanism; the flag
// had nowhere to live in `CommandGroup` and is now a field on it, hashed
// into the chain.
//
// The three tests below are one question asked three ways, because a flag
// that is only ever asserted true proves nothing. What an auditor needs is
// that the flag **discriminates**: it is set when rule 6 was used, and not
// set when it was not — including for the same operation on the same row by
// somebody else, and for the same person doing something else.

/// Ana (`mem_1`) holds `admin` on `board_1` and is a **manager**, not the
/// owner. She reaches `grantBoardsAccess` through the ordinary derived
/// guard, which is the control this whole thing needs: same op, same entity,
/// same row, no rule 6.
const ADMIN_NOT_OWNER: Option<&str> = Some("mem_1");

/// Whether the group this write appended was flagged.
fn flagged(log: &ordo_conformance::WriteLog) -> bool {
    log.last()
        .expect("the write appended a group")
        .owner_administration
}

/// **All three verbs rule 6 names**, and each lands flagged. Grant, revoke
/// and level change *are* the reach the ruling describes, so each one is a
/// command an auditor must be able to recognise as the owner exercising a
/// capability rather than holding a level.
#[test]
fn every_owner_administration_verb_lands_flagged() {
    let (f, pg) = setup("ordo_flag_owner");
    let mut log = WriteLog::new();

    f.run_write_as(
        &pg,
        &mut log,
        r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")"#,
        OWNER,
    )
    .expect("an owner may grant");
    assert!(flagged(&log), "grant by the owner is owner administration");
    assert_eq!(
        log.last().expect("group").actor.to_string(),
        "user:mem_o",
        "the actor half of rule 6 — the flag never replaces attribution"
    );

    f.run_write_as(
        &pg,
        &mut log,
        r#"setBoardsAccessLevel($board: "board_1", $member: "mem_4", $level: "admin")"#,
        OWNER,
    )
    .expect("an owner may change a level");
    assert!(flagged(&log), "level change by the owner");

    f.run_write_as(
        &pg,
        &mut log,
        r#"revokeBoardsAccess($board: "board_1", $member: "mem_4")"#,
        OWNER,
    )
    .expect("an owner may revoke");
    assert!(flagged(&log), "revoke by the owner");
}

/// **The same operation, by somebody who is not the owner, lands
/// unflagged.** Ana reaches the identical op on the identical row by holding
/// `admin` on the board — the ordinary path — and the resulting group is
/// indistinguishable from any other write except in the one way that
/// matters.
///
/// This is the distinction the rule exists to make, and the reason the flag
/// cannot simply be derived at read time from "the actor is the space
/// owner": that question has the same answer whichever path the call took.
#[test]
fn the_same_grant_by_a_non_owner_lands_unflagged() {
    let (f, pg) = setup("ordo_flag_non_owner");
    let mut log = WriteLog::new();

    f.run_write_as(
        &pg,
        &mut log,
        r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")"#,
        ADMIN_NOT_OWNER,
    )
    .expect("Ana is admin of board_1 and reaches this through the guard");
    assert_eq!(level_of(&pg, "board_1", "mem_4").as_deref(), Some("read"));
    assert!(
        !flagged(&log),
        "the ordinary path is not owner administration, however privileged \
         the caller"
    );
    assert_eq!(log.last().expect("group").actor.to_string(), "user:mem_1");
}

/// **A non-acl write by the owner lands unflagged**, which is the scoping
/// half restated in the log. `categories` is a tree nobody guards, so the
/// owner can write it — and does so as an ordinary caller, because rule 6
/// confers nothing there. A flag keyed on "the owner wrote something" rather
/// than on the door's decision would set here, and would be wrong.
#[test]
fn an_ordinary_write_by_the_owner_lands_unflagged() {
    let (f, pg) = setup("ordo_flag_ordinary");
    let mut log = WriteLog::new();

    f.run_write_as(
        &pg,
        &mut log,
        r#"addRootCategory($name: "owner's own category")"#,
        OWNER,
    )
    .expect("`categories` is unguarded; the owner writes it like anyone");
    assert!(
        !flagged(&log),
        "the flag follows the door's rule-6 decision, not the caller's role"
    );
    assert_eq!(log.last().expect("group").actor.to_string(), "user:mem_o");
}

/// The flag is **in the chain**, not beside it: it is covered by the group
/// hash, so a stored owner-administration record cannot be quietly cleared
/// by whoever holds the database. Asserted here on a group the door actually
/// produced, rather than on a synthetic one.
#[test]
fn the_flag_is_covered_by_the_chain_hash() {
    let (f, pg) = setup("ordo_flag_chain");
    let mut log = WriteLog::new();

    f.run_write_as(
        &pg,
        &mut log,
        r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")"#,
        OWNER,
    )
    .expect("an owner may grant");

    let group = log.last().expect("group");
    assert!(group.owner_administration);
    let mut cleared = group.clone();
    cleared.owner_administration = false;
    assert_ne!(
        ordo_commandlog::hash_group(ordo_commandlog::ChainHash::GENESIS, &group),
        ordo_commandlog::hash_group(ordo_commandlog::ChainHash::GENESIS, &cleared),
        "clearing the flag must move the hash, or the record is editable"
    );
}
