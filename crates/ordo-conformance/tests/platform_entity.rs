//! `members` is read-only to domain code — DL §3, and now enforced.
//!
//! Found by an audit, and proven against a real server before the fix: DL §3
//! says a platform entity's fields are *"all read-only to domain code"* and
//! that *"there is no domain-authored path to minting one"*. Nothing enforced
//! it. `ordo_schema::platform_members` built every field as an ordinary
//! writable one, so this compiled, registered and ran:
//!
//! ```text
//! op promote($id: id) = set members { id: $id, role: "owner" }
//! ```
//!
//! The escalation is not theoretical, and it is not bounded by the guard
//! system: `ordo_host::deployment_engine` builds the door's access engine by
//! reading `SELECT "id", "role" FROM "members"`, so **the role a domain op
//! writes is the role the door reads**. The reference profile grants authored
//! write ops to Editor and up, so an editor could promote themselves — and in
//! the audit's run, every other member too — to owner. A bulk form reported
//! `updated: 6`.
//!
//! The class is `reserved_derived_name`, whose documented meaning was already
//! *"the statement writes something the platform owns"*; the inventory row
//! named `id` and `rev` because those were the only two such things.

mod gc_support;

use gc_support::schema_with;

/// Build AND register, because the refusal is a registration gate — the
/// schema alone is happy to hold an entity a statement may not touch.
fn register(extra: &str) -> Result<(), String> {
    let src = format!("{}\n\n{}", ordo_conformance::FIXTURE, extra);
    ordo_conformance::Fixture::from_source(&src, ordo_conformance::SEED).map(|_| ())
}

/// The op the audit used, and three more. None may survive registration.
#[test]
fn a_domain_op_cannot_write_the_platform_principal_entity() {
    for src in [
        r#"op promote($id: id) = set members { id: $id, role: "owner" }"#,
        r#"op promoteAll() = set members(role == "editor") { role: "owner" }"#,
        r#"op erase($id: id) = del members(id == $id)"#,
        r#"op unsuspend($id: id) = set members(id == $id) { status: "active" }"#,
        // The canary asked for this: a second platform entity must inherit
        // the refusal rather than the reasoning being re-derived for it.
        // `groupMembers` is the door's own input — a domain op editing it
        // edits who the door lets in.
        // (`group:` as a brace key does not parse — `group` is a pipeline
        // step keyword — so these use the spellings that do. The refusal is
        // the subject; the keyword collision is noted where it belongs.)
        r#"op join($m: id) = set groupMembers(member == $m) { member: $m }"#,
        r#"op leave($m: id) = del groupMembers(member == $m)"#,
    ] {
        let e = register(src)
            .err()
            .unwrap_or_else(|| panic!("`{src}` must not register"));
        assert!(
            e.contains("reserved_derived_name"),
            "refused by class: `{src}` gave {e}"
        );
        assert!(
            e.contains("members") || e.contains("groupMembers"),
            "the message names the entity: {e}"
        );
    }
}

/// Every verb, because the refusal lives where the statements are walked
/// rather than in one verb's arm — a check the next verb can forget is not a
/// check.
#[test]
fn the_refusal_covers_the_verbs_uniformly() {
    let e = register(r#"op wipe($id: id) = del members { id: $id } subtree"#)
        .expect_err("a subtree delete on a platform entity must not register");
    assert!(e.contains("reserved_derived_name"), "{e}");
}

/// The refusal must be narrow: ordinary entities are untouched. A false
/// positive here would break every write in the language, which is worse
/// than the hole it closes.
#[test]
fn ordinary_entities_are_unaffected() {
    register(r#"op retitle($t: id, $s: text) = set tasks(id == $t) { title: $s }"#)
        .expect("an ordinary authored write still registers");
    // …and the fixture itself, which is the loudest canary available: if this
    // check were wrong about any real entity the whole corpus would stop
    // building.
    schema_with("").expect("the market fixture still builds");
}

/// It is refused at the SCHEMA boundary too, not only when a statement runs
/// — an op that cannot ever be executed should not register.
#[test]
fn the_entity_is_marked_platform_in_the_schema() {
    let s = schema_with("").expect("fixture builds");
    assert!(
        s.entity("members").expect("members exists").platform,
        "`members` must carry the flag the refusal keys on"
    );
    assert!(
        s.entity("groupMembers")
            .expect("groupMembers exists")
            .platform,
        "`groupMembers` is platform-provided on the same footing as `members` \
         (DL §3, Groups) — it is the door's own input, so a domain op minting \
         or deleting membership rows would be editing who the door lets in"
    );
    assert_eq!(
        s.entities.iter().filter(|e| e.platform).count(),
        2,
        "two platform entities today — if that changes, the escalation this \
         file records should be re-run against the new one"
    );
    assert!(
        !s.entity("tasks").expect("tasks exists").platform,
        "an authored entity is not platform-provided"
    );
}
