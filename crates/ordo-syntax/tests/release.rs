//! The `release member` line (checklist 27aq, `plan-member-release.md` §2):
//! whether a member may leave a tenant on their own, and what leaving does to
//! the rows that point at them.

use ordo_syntax::{
    parse_program, GuardAtom, ReassignTarget, ReleaseEffect, ReleaseMode, DUPLICATE_RELEASE,
    RELEASE_SUBJECT,
};

#[test]
fn release_open_locked_and_when_parse() {
    let open = parse_program("release member open\n").expect("open");
    assert!(matches!(
        open.release.as_ref().map(|r| &r.mode),
        Some(ReleaseMode::Open { .. })
    ));

    let locked = parse_program("release member locked").expect("locked, at end of input");
    assert!(matches!(
        locked.release.as_ref().map(|r| &r.mode),
        Some(ReleaseMode::Locked { .. })
    ));

    let when = parse_program(
        "release member when not invoices(owner == $member and status == \"open\") \
         and not projects(lead == $member)\n",
    )
    .expect("when");
    let r = when.release.expect("a rule");
    let ReleaseMode::When(cond) = &r.mode else {
        panic!("when");
    };
    assert_eq!(cond.disjuncts.len(), 1);
    let atoms = &cond.disjuncts[0];
    assert_eq!(atoms.len(), 2);
    assert!(atoms.iter().all(|a| a.negated));
    assert_eq!(atoms[0].entity.name, "invoices");
    // `$member` is stored as the walk's own subject, as `$caller` is in a guard.
    assert!(matches!(
        &atoms[0].inner.disjuncts[0][0],
        GuardAtom::Cmp { caller: true, .. }
    ));
    assert!(r.effects.is_empty());

    // No release line is no rule: the program's members are locked.
    assert!(parse_program("entity a {\n  n: text\n}\n")
        .expect("plain")
        .release
        .is_none());
}

#[test]
fn a_release_block_declares_remove_and_reassign() {
    let p = parse_program(
        "release member open {\n  remove drafts.author\n  \
         reassign tasks.assignee to people(member == $member).manager or owner\n}\n",
    )
    .expect("block");
    let r = p.release.expect("rule");
    assert_eq!(r.effects.len(), 2);
    assert!(matches!(
        &r.effects[0],
        ReleaseEffect::Remove { entity, field, .. } if entity.name == "drafts" && field.name == "author"
    ));
    let ReleaseEffect::Reassign {
        entity, field, to, ..
    } = &r.effects[1]
    else {
        panic!("reassign");
    };
    assert_eq!(
        (entity.name.as_str(), field.name.as_str()),
        ("tasks", "assignee")
    );
    assert_eq!(to.len(), 2);
    assert!(matches!(
        &to[0],
        ReassignTarget::Lookup { entity, field, .. } if entity.name == "people" && field.name == "manager"
    ));
    assert!(matches!(&to[1], ReassignTarget::Owner { .. }));
}

#[test]
fn a_multi_line_rule_is_written_in_parentheses() {
    let p = parse_program(
        "release member when (not invoices(owner == $member)\n  \
         and not projects(lead == $member))\n",
    )
    .expect("a parenthesized rule may span lines");
    let Some(ReleaseMode::When(c)) = p.release.map(|r| r.mode) else {
        panic!("when");
    };
    assert_eq!(c.disjuncts[0].len(), 2);
}

#[test]
fn two_release_lines_are_refused() {
    let e = parse_program("release member open\nrelease member locked\n").unwrap_err();
    assert_eq!(e.code, DUPLICATE_RELEASE, "{e}");
}

/// `$caller` inside a release rule is refused: the rule is about the seat
/// being released, whoever asks. `$member` outside one stays an ordinary op
/// parameter, which existing programs use (`unshareNote($note, $member)`).
#[test]
fn caller_inside_release_is_refused() {
    let e = parse_program("release member when not tasks(assignee == $caller)\n").unwrap_err();
    assert_eq!(e.code, RELEASE_SUBJECT, "{e}");

    let e = parse_program("release member open {\n  reassign tasks.assignee to people(member == $caller).manager\n}\n")
        .unwrap_err();
    assert_eq!(e.code, RELEASE_SUBJECT, "{e}");

    // A guard outside the rule keeps `$caller`, and an op keeps `$member`.
    // The rule after an op's body is a declaration of its own.
    let p = parse_program(
        "entity tasks {\n  assignee -> members\n  access read assignee == $caller\n  \
         access write assignee == $caller\n}\n\
         op unassign($member: id) =\n  del tasks(assignee == $member)\n\
         release member when not tasks(assignee == $member)\n",
    )
    .expect("both subjects in their own places");
    assert!(p.release.is_some());
    assert_eq!(p.ops.len(), 1);
}

#[test]
fn not_binds_to_one_condition() {
    let e = parse_program("release member when not (a(x == $member) or b(y == $member))\n")
        .unwrap_err();
    assert!(e.message.contains("`not` applies to one condition"), "{e}");
}

#[test]
fn release_stays_usable_as_a_name() {
    parse_program(
        "entity versions {\n  release: text\n}\n\
         op release($v: id) =\n  versions(id == $v)[release]\n",
    )
    .expect("`release` is contextual: a field and an op may still be called it");
}
