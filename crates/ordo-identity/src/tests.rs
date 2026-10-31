//! Unit tests for the identity crate: access-check layers, capability
//! strictness, step-up gating, provider round-trips, and audited linking.

use std::collections::BTreeSet;

use crate::access::{AccessEngine, Decision, DenyReason, StepUpRequirement, TokenClaims};
use crate::credentials::CredentialStore as _;
use crate::directory::{
    AuditEventKind, Directory, DirectoryError, DirectoryStore, LinkCeremony, MemoryDirectoryStore,
    Provisioning,
};
use crate::pins::{decode_pins, encode_pins, EntityPin, GuardPin, OpPin, PinDrift};
use crate::principal::{
    AccessProfile, AuthMethodKind, ClientId, GrantId, GroupId, ManifestHash, MembershipStatus,
    PrincipalId, PrincipalKind, Role, SpaceId, Timestamp,
};
use crate::provider::{
    AuthError, AuthProvider, AuthenticatorId, BeginRequest, ChallengeOrRedirect, CompleteRequest,
    LocalPasswordProvider, PlainTestHasher, StubOidcProvider,
};
use crate::registry::{ProviderRegistry, RegistryError};
use crate::session::SessionRevocations as _;

const T0: Timestamp = Timestamp(1_000);
const T1: Timestamp = Timestamp(2_000);
/// The server clock a provider ceremony runs at. Named apart from `T0` so a
/// reader of a provider test sees which instant is the *ceremony's*.
const NOW: Timestamp = Timestamp(10_000);

fn space() -> SpaceId {
    SpaceId("spc_01".into())
}

/// A second space in the same deployment — the one a space-blind grant or a
/// space-blind lookup would wrongly reach.
fn other_space() -> SpaceId {
    SpaceId("spc_02".into())
}

fn user(n: &str) -> PrincipalId {
    PrincipalId(format!("usr_{n}"))
}

fn group(n: &str) -> GroupId {
    GroupId(format!("grp_{n}"))
}

fn ops(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn human_claims(p: &PrincipalId) -> TokenClaims {
    TokenClaims {
        principal: p.clone(),
        capability: None,
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    }
}

/// An engine with one space profile: viewers read, editors read+write,
/// owners read+write+admin. Nothing implicit — the profile is the law.
fn engine_with_profile() -> AccessEngine {
    let mut e = AccessEngine::new();
    let profile = AccessProfile::new()
        .allow(Role::Viewer, "txns.list")
        .allow(Role::Editor, "txns.list")
        .allow(Role::Editor, "txns.setCategory")
        .allow(Role::Owner, "txns.list")
        .allow(Role::Owner, "txns.setCategory")
        .allow(Role::Owner, "space.delete");
    e.set_profile(&space(), profile);
    e
}

// ───────────────────────── role/profile resolution ──────────────────────

#[test]
fn role_profile_resolution() {
    let mut e = engine_with_profile();
    let viewer = user("v");
    let editor = user("e");
    let owner = user("o");
    e.set_membership(&space(), &viewer, Role::Viewer);
    e.set_membership(&space(), &editor, Role::Editor);
    e.set_membership(&space(), &owner, Role::Owner);

    assert!(e
        .check(&human_claims(&viewer), &space(), "txns.list", T0)
        .is_allow());
    assert_eq!(
        e.check(&human_claims(&viewer), &space(), "txns.setCategory", T0),
        Decision::Deny {
            op: "txns.setCategory".into(),
            reason: DenyReason::OpNotInRole { role: Role::Viewer },
        }
    );
    assert!(e
        .check(&human_claims(&editor), &space(), "txns.setCategory", T0)
        .is_allow());
    assert!(e
        .check(&human_claims(&owner), &space(), "space.delete", T0)
        .is_allow());
}

#[test]
fn no_membership_denies_with_op() {
    let e = engine_with_profile();
    let stranger = user("s");
    assert_eq!(
        e.check(&human_claims(&stranger), &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::NoMembership
        }
    );
}

/// A seat with no role is a guest (`plan-sharing.md` §3.1): it binds a
/// member id, reaches no op by role, and is refused on the human path with
/// its own reason, ahead of the profile.
#[test]
fn a_seat_with_no_role_is_a_guest_and_is_refused_by_role() {
    let mut e = engine_with_profile();
    let g = user("guest");
    e.set_member(&space(), &g, "mem_guest", None);

    assert_eq!(e.member_id_of(&space(), &g), Some("mem_guest"));
    assert_eq!(
        e.spaces_of(&g),
        vec![(space(), None, MembershipStatus::Active)]
    );
    assert!(!e.membership_reaches(&space(), &g, "txns.list"));
    assert_eq!(
        e.check(&human_claims(&g), &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::NoRole
        }
    );
    // Not even a space with no profile changes the reason: the seat is
    // refused before the profile is read.
    let mut bare = AccessEngine::new();
    bare.set_member(&space(), &g, "mem_guest", None);
    assert_eq!(
        bare.check(&human_claims(&g), &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::NoRole
        }
    );
    // Promotion is `set_member` with a role: the seat and its id stay.
    e.set_member(&space(), &g, "mem_guest", Some(Role::Viewer));
    assert!(e
        .check(&human_claims(&g), &space(), "txns.list", T0)
        .is_allow());
}

/// A guest is never a role, group or override caller of an op.
#[test]
fn a_guest_is_not_listed_as_a_caller_by_role() {
    let mut e = engine_with_profile();
    let g = user("guest");
    e.set_member(&space(), &g, "mem_guest", None);
    assert!(e.callers_of(&space(), "txns.list", &[], &[], T0).is_empty());
}

#[test]
fn no_profile_denies() {
    let mut e = AccessEngine::new();
    let u = user("u");
    e.set_membership(&space(), &u, Role::Owner);
    assert_eq!(
        e.check(&human_claims(&u), &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::NoProfileForSpace
        }
    );
}

#[test]
fn roles_have_no_implicit_hierarchy() {
    // Owner is not listed for an op → owner is denied. Templates ship
    // owner-sees-everything; the engine does not.
    let mut e = AccessEngine::new();
    let profile = AccessProfile::new().allow(Role::Viewer, "txns.list");
    e.set_profile(&space(), profile);
    let o = user("o");
    e.set_membership(&space(), &o, Role::Owner);
    assert_eq!(
        e.check(&human_claims(&o), &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::OpNotInRole { role: Role::Owner },
        }
    );
}

// ───────────────────────── capability allowlists ────────────────────────

/// A pin over one entity, spelled the way a compiled op would produce it.
/// `ordo-identity` never compiles anything, so these tests state the pin
/// directly; `crates/ordo-host/tests/pinning.rs` is where the same shapes
/// come out of a real program against a real database.
fn pin(seed: &[u8], read: &[&str], guard: Option<&str>, caller: bool) -> OpPin {
    OpPin {
        manifest_hash: ManifestHash::of(seed),
        unguarded: false,
        entities: vec![EntityPin {
            entity: "txns".into(),
            read: read.iter().map(|s| s.to_string()).collect(),
            written: BTreeSet::new(),
            structure: false,
            read_guard: guard.map_or(GuardPin::Every, |g| GuardPin::Walk(g.into())),
            write_guard: GuardPin::Every,
            caller_restricted: caller,
        }],
    }
}

fn base_pin(seed: &[u8]) -> OpPin {
    pin(seed, &["amount", "payee"], Some("owner == $caller"), true)
}

/// Engine + a grant over the given ops **in `grant_space`**, freshly
/// consented at current deploy.
fn engine_with_grant_in(grant_space: &SpaceId, grant_ops: &[&str]) -> (AccessEngine, TokenClaims) {
    let mut e = engine_with_profile();
    e.deploy_op("txns.list", base_pin(b"txns.list v1"));
    e.deploy_op("txns.setCategory", base_pin(b"txns.setCategory v1"));
    let grant = e
        .grant_for(GrantId("gr_test".into()), grant_space, ops(grant_ops), T1)
        .expect("all ops deployed");
    let claims = TokenClaims {
        principal: PrincipalId("agt_ellanor".into()),
        capability: Some(grant),
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };
    (e, claims)
}

fn engine_with_grant(grant_ops: &[&str]) -> (AccessEngine, TokenClaims) {
    engine_with_grant_in(&space(), grant_ops)
}

// ───────── capability grants are space-scoped (SH2-2 / Y-002) ────────────

#[test]
fn grant_for_one_space_is_denied_in_another() {
    // The whole of Y-002: op names are deployment-wide, so a grant consented
    // in space A must not be currency in space B just because the caller
    // named B's id.
    let (e, claims) = engine_with_grant_in(&space(), &["txns.list"]);
    assert_eq!(
        e.check(&claims, &other_space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::GrantSpaceMismatch
        }
    );
}

#[test]
fn grant_is_allowed_in_its_own_space() {
    let (e, claims) = engine_with_grant_in(&space(), &["txns.list"]);
    assert!(e.check(&claims, &space(), "txns.list", T0).is_allow());
    // …and a grant minted for the *other* space is the mirror image.
    let (e2, claims2) = engine_with_grant_in(&other_space(), &["txns.list"]);
    assert!(e2
        .check(&claims2, &other_space(), "txns.list", T0)
        .is_allow());
    assert_eq!(
        e2.check(&claims2, &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::GrantSpaceMismatch
        }
    );
}

#[test]
fn wrong_space_is_checked_before_expiry_and_allowlist() {
    // Space is the first question the capability path asks: an expired grant
    // for an op it never held still denies as a space mismatch, so the
    // isolating check cannot be shadowed by a cheaper failure.
    let (e, claims) = engine_with_grant_in(&space(), &["txns.list"]);
    assert_eq!(
        e.check(&claims, &other_space(), "txns.setCategory", T1),
        Decision::Deny {
            op: "txns.setCategory".into(),
            reason: DenyReason::GrantSpaceMismatch
        }
    );
}

#[test]
fn capability_allowlist_is_strict() {
    let (e, claims) = engine_with_grant(&["txns.list"]);
    assert!(e.check(&claims, &space(), "txns.list", T0).is_allow());
    assert_eq!(
        e.check(&claims, &space(), "txns.setCategory", T0),
        Decision::Deny {
            op: "txns.setCategory".into(),
            reason: DenyReason::OpNotInAllowlist
        }
    );
}

#[test]
fn capability_never_falls_through_to_roles() {
    // Even when the same principal holds an Owner membership, a capability
    // token's grant is the entire authority.
    let (mut e, claims) = engine_with_grant(&["txns.list"]);
    e.set_membership(&space(), &claims.principal, Role::Owner);
    assert_eq!(
        e.check(&claims, &space(), "space.delete", T0),
        Decision::Deny {
            op: "space.delete".into(),
            reason: DenyReason::OpNotInAllowlist
        }
    );
}

#[test]
fn expired_grant_denies() {
    let (e, claims) = engine_with_grant(&["txns.list"]);
    // Just before expiry: allowed. At the expiry instant: expired.
    assert!(e
        .check(&claims, &space(), "txns.list", Timestamp(T1.0 - 1))
        .is_allow());
    assert_eq!(
        e.check(&claims, &space(), "txns.list", T1),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::GrantExpired
        }
    );
}

#[test]
fn manifest_change_kills_grant_until_reconsent() {
    let (mut e, claims) = engine_with_grant(&["txns.list"]);
    // Redeploy expands the op's scope: a field the consent never mentioned.
    e.deploy_op(
        "txns.list",
        pin(
            b"txns.list v2 wider scope",
            &["amount", "note", "payee"],
            Some("owner == $caller"),
            true,
        ),
    );
    assert_eq!(
        e.check(&claims, &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::ManifestChanged
        }
    );
    // Re-consent pins the new hash → alive again.
    let reconsented = e
        .grant_for(GrantId("gr_test".into()), &space(), ops(&["txns.list"]), T1)
        .expect("still deployed");
    let claims2 = TokenClaims {
        capability: Some(reconsented),
        ..claims
    };
    assert!(e.check(&claims2, &space(), "txns.list", T0).is_allow());
}

#[test]
fn undeployed_op_reads_as_manifest_change() {
    let mut e = AccessEngine::new();
    e.deploy_op("txns.list", base_pin(b"v1"));
    let grant = e
        .grant_for(GrantId("gr_test".into()), &space(), ops(&["txns.list"]), T1)
        .unwrap();
    // Simulate the op vanishing from the deployment: fresh engine without it.
    let e2 = AccessEngine::new();
    let claims = TokenClaims {
        principal: PrincipalId("agt_a".into()),
        capability: Some(grant),
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };
    assert_eq!(
        e2.check(&claims, &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::ManifestChanged
        }
    );
}

#[test]
fn grant_for_unknown_op_is_unmintable() {
    let e = AccessEngine::new();
    assert_eq!(
        e.grant_for(GrantId("gr_test".into()), &space(), ops(&["ghost.op"]), T1),
        None
    );
}

#[test]
fn manifest_hash_is_sha256_not_a_64_bit_fold() {
    // SH-27: consent integrity rests on collision resistance, so the digest
    // is the full 256 bits and differs on a one-byte change.
    let a = ManifestHash::of(b"txns.list v1");
    let b = ManifestHash::of(b"txns.list v2");
    assert_ne!(a, b);
    assert_eq!(a.0.len(), 32);
    assert_ne!(a.0, [0u8; 32]);
}

// ───────── the pin comparison: DL §8's counterexample and its twin ───────
//
// These are the unit-level statements of the ruling. The end-to-end proofs —
// a real program, edited, recompiled and re-deployed against real Postgres —
// are `crates/ordo-host/tests/pinning.rs`. Both exist on purpose: this file
// says what the rule IS, that one says the compiler actually produces the
// shape the rule needs.

/// An engine holding one grant over `txns.list`, pinned at `before`, now
/// deploying `after`.
fn redeployed(before: OpPin, after: OpPin) -> (AccessEngine, TokenClaims, PinDrift) {
    let mut e = engine_with_profile();
    e.deploy_op("txns.list", before);
    let grant = e
        .grant_for(GrantId("gr_test".into()), &space(), ops(&["txns.list"]), T1)
        .expect("deployed");
    e.deploy_op("txns.list", after);
    let drift = e.drift(&grant).remove(0).1;
    let claims = TokenClaims {
        principal: PrincipalId("agt_ellanor".into()),
        capability: Some(grant),
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };
    (e, claims, drift)
}

fn denied() -> Decision {
    Decision::Deny {
        op: "txns.list".into(),
        reason: DenyReason::ManifestChanged,
    }
}

/// **The counterexample `definition-language.md` §8 is built on, closed.**
///
/// > Delete `member == $caller` from an op body … and the op's output field
/// > set is byte-identical: same columns, same types, same order. A
/// > field-set comparison reads that as "not an expansion", classifies it as
/// > narrowing, and applies it silently.
///
/// So the field sets here are **identical** — deliberately, and the test
/// asserts it before asserting anything else. The op projected the very
/// field it filtered on, which is the ordinary case (`txns(owner ==
/// $caller)[amount, payee, owner]`), so deleting the filter moves no column
/// at all. Every input the losing form of the rule can see is unchanged.
///
/// The grant must die anyway.
#[test]
fn deleting_a_caller_restriction_kills_the_grant_though_no_column_moved() {
    let before = pin(b"v1", &["amount", "payee"], Some("owner == $caller"), true);
    let after = pin(b"v2", &["amount", "payee"], Some("owner == $caller"), false);

    // The premise. If this ever stops holding, the test below has stopped
    // being about the thing it is named for.
    assert_eq!(
        before.entities[0].read, after.entities[0].read,
        "the counterexample requires a byte-identical field set"
    );
    assert_eq!(before.entities[0].read_guard, after.entities[0].read_guard);
    assert_eq!(before.unguarded, after.unguarded);

    let (e, claims, drift) = redeployed(before, after);
    assert!(
        matches!(drift, PinDrift::Expanded { .. }),
        "a lost `$caller` restriction is an expansion, not a narrowing: {drift:?}"
    );
    assert!(
        drift.note().contains("`$caller` restriction"),
        "the audit note must say what widened: {}",
        drift.note()
    );
    assert_eq!(e.check(&claims, &space(), "txns.list", T0), denied());
}

/// **The other half of the same sentence**: *"or add `unguarded` to its
/// header"*. Same field set, same guard declared on the entity, same
/// `$caller` restriction — one flag, and every guard on the op's own sources
/// stops applying.
///
/// This is the case a field-set comparison is *most* blind to, because
/// bypassing a guard makes the field set **smaller** (the guard-reached
/// entities drop out of the manifest), so the losing rule reads a total
/// authorization bypass as a narrowing and applies it silently.
#[test]
fn adding_unguarded_kills_the_grant_though_the_field_set_only_shrank() {
    let before = pin(b"v1", &["amount", "payee"], Some("owner == $caller"), true);
    let mut after = pin(b"v2", &["amount"], Some("owner == $caller"), true);
    after.unguarded = true;

    assert!(
        after.entities[0].read.is_subset(&before.entities[0].read),
        "the flag makes the field set a strict SUBSET — which is what a \
         field-set-only rule calls narrowing"
    );

    let (e, claims, drift) = redeployed(before, after);
    assert!(
        matches!(drift, PinDrift::Expanded { .. }),
        "`unguarded` newly set is an expansion: {drift:?}"
    );
    assert!(drift.note().contains("unguarded"), "{}", drift.note());
    assert_eq!(e.check(&claims, &space(), "txns.list", T0), denied());
}

/// A guard that stops applying to the entity — the same widening reached by
/// editing the schema instead of the op header.
#[test]
fn dropping_an_entitys_guard_kills_the_grant() {
    let before = pin(b"v1", &["amount"], Some("owner == $caller"), false);
    let after = pin(b"v2", &["amount"], None, false);
    let (e, claims, drift) = redeployed(before, after);
    assert!(matches!(drift, PinDrift::Expanded { .. }), "{drift:?}");
    assert!(
        drift.note().contains("lost its read guard"),
        "{}",
        drift.note()
    );
    assert_eq!(e.check(&claims, &space(), "txns.list", T0), denied());
}

/// A guard **rewritten** rather than removed. Undecidable to call narrower —
/// deciding it is query containment — so it is fail-closed by rule, and DL
/// says so in as many words: *"is the same walk"*.
#[test]
fn rewriting_a_guard_walk_kills_the_grant_even_when_it_looks_tighter() {
    let before = pin(b"v1", &["amount"], Some("owner == $caller"), false);
    let after = pin(
        b"v2",
        &["amount"],
        Some("owner == $caller and approved"),
        false,
    );
    let (_, claims, drift) = redeployed(before, after);
    assert!(matches!(drift, PinDrift::Expanded { .. }), "{drift:?}");
    let (e, _, _) = (engine_with_profile(), 0, 0);
    let _ = (e, claims);
}

/// **Narrowing applies silently** — the behaviour §5 wanted and the combined
/// hash could not deliver, because narrowing changes the hash too.
///
/// Three narrowings at once, each of a different kind: fewer fields, a guard
/// where there was none, and a `$caller` restriction where there was none.
/// The call is ALLOWED, and the drift report says what shrank so the
/// narrowing can be audit-noted.
#[test]
fn narrowing_applies_silently_and_is_audit_noted() {
    let before = pin(b"v1", &["amount", "note", "payee"], None, false);
    let after = pin(b"v2", &["amount"], Some("owner == $caller"), true);
    let (e, claims, drift) = redeployed(before, after);

    assert!(
        matches!(drift, PinDrift::Narrowed { .. }),
        "less reach in every dimension is narrowing: {drift:?}"
    );
    let note = drift.note();
    for expected in ["fewer fields", "gained a read guard", "gained a `$caller`"] {
        assert!(note.contains(expected), "note missing `{expected}`: {note}");
    }
    assert!(
        e.check(&claims, &space(), "txns.list", T0).is_allow(),
        "a person who consented to more must not be asked again for less"
    );
}

/// Clearing `unguarded` is narrowing too: the guards come back.
#[test]
fn clearing_unguarded_is_narrowing() {
    let mut before = pin(b"v1", &["amount"], Some("owner == $caller"), false);
    before.unguarded = true;
    let after = pin(b"v2", &["amount"], Some("owner == $caller"), false);
    let (e, claims, drift) = redeployed(before, after);
    assert!(matches!(drift, PinDrift::Narrowed { .. }), "{drift:?}");
    assert!(e.check(&claims, &space(), "txns.list", T0).is_allow());
}

/// **The per-op half of the ruling.** An unrelated op *narrowing* must not
/// invalidate a consent — that is exactly what one combined hash could not
/// express, because narrowing changes the hash.
///
/// And the other direction, in the same test: an unrelated op **expanding**
/// does kill the grant. Consent was one act over a set of ops, and DL §8 says
/// an expansion makes the grant *"dead for that space until re-consent"* —
/// per-op is the granularity of the COMPARISON, not a licence for a grant to
/// stay half-alive.
#[test]
fn an_unrelated_ops_narrowing_is_survivable_and_its_expansion_is_not() {
    let mut e = engine_with_profile();
    e.deploy_op("txns.list", base_pin(b"list v1"));
    e.deploy_op("txns.setCategory", base_pin(b"setCategory v1"));
    let grant = e
        .grant_for(
            GrantId("gr_test".into()),
            &space(),
            ops(&["txns.list", "txns.setCategory"]),
            T1,
        )
        .expect("deployed");
    let claims = TokenClaims {
        principal: PrincipalId("agt_ellanor".into()),
        capability: Some(grant),
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };

    // `txns.setCategory` narrows. The call to `txns.list` is untouched.
    e.deploy_op(
        "txns.setCategory",
        pin(
            b"setCategory v2",
            &["amount"],
            Some("owner == $caller"),
            true,
        ),
    );
    assert!(
        e.check(&claims, &space(), "txns.list", T0).is_allow(),
        "a combined hash denied here, and that is the bug §5 named"
    );

    // Now it expands. The whole grant dies, including the untouched op.
    e.deploy_op(
        "txns.setCategory",
        pin(
            b"setCategory v3",
            &["amount", "payee"],
            Some("owner == $caller"),
            false,
        ),
    );
    assert_eq!(e.check(&claims, &space(), "txns.list", T0), denied());
}

/// A change the pinned shape does not cover — same columns, same rows, a
/// different manifest — applies, and says so rather than claiming something
/// narrowed. Recorded because it is the residue of DL's rule: the pin
/// answers *which columns* and *which rows*, and a `take` bound is neither.
#[test]
fn a_change_outside_the_pinned_shape_applies_and_names_itself() {
    let before = base_pin(b"v1");
    let after = base_pin(b"v2 - same shape, different cost class");
    let (e, claims, drift) = redeployed(before, after);
    assert!(matches!(drift, PinDrift::Narrowed { .. }), "{drift:?}");
    assert!(
        drift.note().contains("outside the pinned shape"),
        "{}",
        drift.note()
    );
    assert!(e.check(&claims, &space(), "txns.list", T0).is_allow());
}

/// A newly reached entity is an expansion even when every field the consent
/// named is still exactly what it was.
#[test]
fn a_newly_reached_entity_kills_the_grant() {
    let before = base_pin(b"v1");
    let mut after = base_pin(b"v2");
    after.entities.push(EntityPin {
        entity: "members".into(),
        read: BTreeSet::from(["role".to_string()]),
        written: BTreeSet::new(),
        structure: false,
        read_guard: GuardPin::Every,
        write_guard: GuardPin::Every,
        caller_restricted: false,
    });
    let (e, claims, drift) = redeployed(before, after);
    assert!(matches!(drift, PinDrift::Expanded { .. }), "{drift:?}");
    assert!(drift.note().contains("newly reached"), "{}", drift.note());
    assert_eq!(e.check(&claims, &space(), "txns.list", T0), denied());
}

/// The encoding a token carries is **injective**: no two different pin sets
/// produce the same bytes.
///
/// This is the property `combined_manifest_hash` needed length prefixes for
/// (SH-27), moved to where the pin now lives. Its inputs are op names, entity
/// names, field names and **guard text**, every one of which a package author
/// controls, and its output decides whether a consented grant stays alive. So
/// the boundaries are pushed at each joint an author could try to walk
/// across: where one op name ends and the next begins, where a field list
/// ends, and where a guard walk ends.
#[test]
fn the_pin_encoding_has_no_ambiguous_boundaries() {
    let map = |entries: &[(&str, OpPin)]| {
        entries
            .iter()
            .map(|(n, p)| (n.to_string(), p.clone()))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let walk = |w: &str| pin(b"same", &["f"], Some(w), false);

    // Op-name boundary: {"ab","c"} and {"a","bc"} are different allowlists.
    let a = map(&[("ab", walk("x")), ("c", walk("x"))]);
    let b = map(&[("a", walk("x")), ("bc", walk("x"))]);
    assert_ne!(encode_pins(&a), encode_pins(&b));

    // Guard-text boundary: a walk that is a prefix of another walk.
    let c = map(&[("op", walk("x"))]);
    let d = map(&[("op", walk("x1"))]);
    assert_ne!(encode_pins(&c), encode_pins(&d));

    // Field-list boundary: {"ab"} and {"a","b"} are different field sets, and
    // the comma-joined form must not collapse them.
    let e = map(&[("op", pin(b"same", &["ab"], None, false))]);
    let f = map(&[("op", pin(b"same", &["a", "b"], None, false))]);
    assert_ne!(encode_pins(&e), encode_pins(&f));

    for m in [a, b, c, d, e, f] {
        assert_eq!(decode_pins(&encode_pins(&m)), Some(m), "round trip");
    }
}

/// A malformed pin blob is refused, never partly understood.
///
/// The dangerous shape is a **truncation**: bytes chopped off the end that
/// still parse as a shorter, weaker pin. A pin that decoded to "one entity,
/// no guard" from a blob that was written as "one entity, guarded" would be a
/// row bound an attacker deleted with a text editor. Counts and total length
/// are both checked, so there is no prefix of a valid blob that is itself
/// valid.
#[test]
fn a_pin_blob_that_is_not_exactly_what_was_written_is_refused() {
    let m = std::collections::BTreeMap::from([("op".to_string(), base_pin(b"v1"))]);
    let good = encode_pins(&m);
    assert_eq!(decode_pins(&good), Some(m));
    for bad in [
        String::new(),
        "7:pins.v1".to_string(),
        format!("{good}x"),
        good.replacen("pins.v1", "pins.v2", 1),
    ] {
        assert_eq!(decode_pins(&bad), None, "{bad:?} decoded");
    }
    // Every proper prefix. Not one of them is a valid shorter pin.
    for n in 0..good.len() {
        assert_eq!(
            decode_pins(&good[..n]),
            None,
            "prefix of length {n} decoded"
        );
    }
}

// ───────────────────────────── step-up ──────────────────────────────────

#[test]
fn step_up_gates_listed_ops() {
    let mut e = engine_with_profile();
    e.require_step_up("space.delete");
    let o = user("o");
    e.set_membership(&space(), &o, Role::Owner);

    let absent = human_claims(&o);
    assert_eq!(
        e.check(&absent, &space(), "space.delete", T0),
        Decision::Deny {
            op: "space.delete".into(),
            reason: DenyReason::StepUpRequired
        }
    );
    let fresh = TokenClaims {
        user_presence_at: Some(T0),
        ..absent
    };
    assert!(e.check(&fresh, &space(), "space.delete", T0).is_allow());
    // Non-step-up ops are untouched by presence.
    assert!(e
        .check(&human_claims(&o), &space(), "txns.list", T0)
        .is_allow());
}

#[test]
fn stale_presence_does_not_satisfy_step_up() {
    // SH-18: presence is a *moment*, not a bit. A token minted with presence
    // long ago gated the login, not this op.
    let mut e = engine_with_profile().with_step_up_max_age(60);
    e.require_step_up("space.delete");
    let o = user("o");
    e.set_membership(&space(), &o, Role::Owner);
    let at = Timestamp(10_000);
    let claims = TokenClaims {
        user_presence_at: Some(at),
        ..human_claims(&o)
    };

    // Inside the window: allowed, at the boundary instant included.
    assert!(e
        .check(&claims, &space(), "space.delete", Timestamp(at.0 + 60))
        .is_allow());
    // One second past it: a typed, loud refusal naming both ages.
    assert_eq!(
        e.check(&claims, &space(), "space.delete", Timestamp(at.0 + 61)),
        Decision::Deny {
            op: "space.delete".into(),
            reason: DenyReason::StepUpStale {
                age_secs: 61,
                max_age_secs: 60
            },
        }
    );
    // A presence stamped in the future is a clock lie, not freshness.
    assert!(matches!(
        e.check(&claims, &space(), "space.delete", Timestamp(at.0 - 1)),
        Decision::Deny {
            reason: DenyReason::StepUpStale { .. },
            ..
        }
    ));
}

#[test]
fn step_up_denial_comes_after_inclusion() {
    // Op both unlisted and step-up-gated → the denial names the real gap
    // (not in role), not step-up.
    let mut e = engine_with_profile();
    e.require_step_up("space.delete");
    let v = user("v");
    e.set_membership(&space(), &v, Role::Viewer);
    assert_eq!(
        e.check(&human_claims(&v), &space(), "space.delete", T0),
        Decision::Deny {
            op: "space.delete".into(),
            reason: DenyReason::OpNotInRole { role: Role::Viewer },
        }
    );
}

#[test]
fn capability_tokens_cannot_satisfy_step_up() {
    let mut e = AccessEngine::new();
    e.deploy_op("transfer.approve", base_pin(b"v1"));
    e.require_step_up("transfer.approve");
    let grant = e
        .grant_for(
            GrantId("gr_test".into()),
            &space(),
            ops(&["transfer.approve"]),
            T1,
        )
        .unwrap();
    let claims = TokenClaims {
        principal: PrincipalId("agt_a".into()),
        capability: Some(grant),
        client: None,
        // Presence is not delegable — and even a token that *claims* a fresh
        // one is refused, because the refusal is structural, not a convention
        // about what minters put in the claim.
        user_presence_at: Some(T0),
        delegation: None,
        session: None,
    };
    assert_eq!(
        e.check(&claims, &space(), "transfer.approve", T0),
        Decision::Deny {
            op: "transfer.approve".into(),
            reason: DenyReason::StepUpRequired
        }
    );
}

// ────────── Z-003: step-up belongs to the op, not to the client ─────────

fn client(n: &str) -> ClientId {
    ClientId(format!("cli_{n}"))
}

/// The attack: two registered relying parties, one of which never listed the
/// op. Presenting the weaker client's token must not turn the deletion gate
/// into a client preference.
#[test]
fn a_permissive_client_cannot_lower_an_ops_step_up_requirement() {
    let mut e = engine_with_profile();
    e.require_step_up("space.delete"); // the op's own minimum
    let pwa = client("appWeb");
    let legacy = client("appLegacyMobile");
    // The main PWA lists it; the older mobile client does not list it at all.
    e.client_requires_step_up(&pwa, "space.delete", StepUpRequirement::PRESENCE);

    let o = user("o");
    e.set_membership(&space(), &o, Role::Owner);
    let through = |c: &ClientId| TokenClaims {
        client: Some(c.clone()),
        ..human_claims(&o)
    };

    for c in [&pwa, &legacy] {
        assert_eq!(
            e.check(&through(c), &space(), "space.delete", T0),
            Decision::Deny {
                op: "space.delete".into(),
                reason: DenyReason::StepUpRequired
            },
            "client `{c}` must not be able to reach a step-up op without presence"
        );
    }
    // A token with no client claim at all is no weaker either.
    assert_eq!(
        e.check(&human_claims(&o), &space(), "space.delete", T0),
        Decision::Deny {
            op: "space.delete".into(),
            reason: DenyReason::StepUpRequired
        }
    );
    // With presence, every client passes — the requirement was never about
    // which app called, only about what was proved.
    let fresh = TokenClaims {
        user_presence_at: Some(T0),
        ..through(&legacy)
    };
    assert!(e.check(&fresh, &space(), "space.delete", T0).is_allow());
}

#[test]
fn a_client_may_add_a_requirement_the_op_does_not_have() {
    let mut e = engine_with_profile();
    let strict = client("appKiosk");
    e.client_requires_step_up(&strict, "txns.setCategory", StepUpRequirement::PRESENCE);
    let ed = user("e");
    e.set_membership(&space(), &ed, Role::Editor);

    // Through an ordinary client the op needs nothing extra.
    assert!(e
        .check(&human_claims(&ed), &space(), "txns.setCategory", T0)
        .is_allow());
    // Through the strict one it does.
    let via_kiosk = TokenClaims {
        client: Some(strict),
        ..human_claims(&ed)
    };
    assert_eq!(
        e.check(&via_kiosk, &space(), "txns.setCategory", T0),
        Decision::Deny {
            op: "txns.setCategory".into(),
            reason: DenyReason::StepUpRequired
        }
    );
}

#[test]
fn requirements_combine_by_max_so_the_shorter_window_wins() {
    // `strongest` is the whole rule, so it is asserted directly as well as
    // through the engine.
    let none = StepUpRequirement::None;
    let wide = StepUpRequirement::Presence { max_age_secs: 300 };
    let tight = StepUpRequirement::Presence { max_age_secs: 30 };
    assert_eq!(none.strongest(none), none);
    assert_eq!(none.strongest(wide), wide);
    assert_eq!(wide.strongest(none), wide);
    assert_eq!(wide.strongest(tight), tight);
    assert_eq!(tight.strongest(wide), tight);

    let mut e = engine_with_profile();
    e.require_step_up_within("space.delete", 300);
    let kiosk = client("appKiosk");
    e.client_requires_step_up(
        &kiosk,
        "space.delete",
        StepUpRequirement::Presence { max_age_secs: 30 },
    );
    assert_eq!(
        e.step_up_for("space.delete", Some(&kiosk)),
        StepUpRequirement::Presence { max_age_secs: 30 }
    );

    let o = user("o");
    e.set_membership(&space(), &o, Role::Owner);
    let at = Timestamp(10_000);
    let claims = TokenClaims {
        client: Some(kiosk),
        user_presence_at: Some(at),
        ..human_claims(&o)
    };
    assert!(e
        .check(&claims, &space(), "space.delete", Timestamp(at.0 + 30))
        .is_allow());
    assert_eq!(
        e.check(&claims, &space(), "space.delete", Timestamp(at.0 + 31)),
        Decision::Deny {
            op: "space.delete".into(),
            reason: DenyReason::StepUpStale {
                age_secs: 31,
                max_age_secs: 30
            },
        },
        "the client tightened the window and the door honored it"
    );
}

#[test]
fn the_deployment_window_is_a_ceiling_no_op_can_raise() {
    // An op that asks for a longer window than the deployment allows gets the
    // deployment's — requirements only ever compose downward in age.
    let mut e = engine_with_profile().with_step_up_max_age(60);
    e.require_step_up_within("space.delete", 3_600);
    let o = user("o");
    e.set_membership(&space(), &o, Role::Owner);
    let at = Timestamp(10_000);
    let claims = TokenClaims {
        user_presence_at: Some(at),
        ..human_claims(&o)
    };
    assert_eq!(
        e.check(&claims, &space(), "space.delete", Timestamp(at.0 + 61)),
        Decision::Deny {
            op: "space.delete".into(),
            reason: DenyReason::StepUpStale {
                age_secs: 61,
                max_age_secs: 60
            },
        }
    );
}

// ───────── Z-005: suspension is an input to the check, not a label ───────

#[test]
fn a_suspended_member_is_denied_ahead_of_role_and_profile() {
    let mut e = engine_with_profile();
    let ed = user("e");
    e.set_membership(&space(), &ed, Role::Editor);
    assert!(e
        .check(&human_claims(&ed), &space(), "txns.list", T0)
        .is_allow());

    e.suspend(&space(), &ed);
    assert_eq!(
        e.membership_status(&space(), &ed),
        Some(MembershipStatus::Suspended)
    );
    assert_eq!(
        e.check(&human_claims(&ed), &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::PrincipalSuspended
        },
        "an already-minted token stops working the moment the status flips"
    );

    // Reversible, and it restores exactly the role that was there.
    e.reactivate(&space(), &ed);
    assert!(e
        .check(&human_claims(&ed), &space(), "txns.list", T0)
        .is_allow());
}

#[test]
fn suspension_is_per_space_and_distinct_from_absence() {
    let mut e = engine_with_profile();
    let ed = user("e");
    e.set_membership(&space(), &ed, Role::Editor);
    e.suspend(&space(), &ed);
    // Another space: no membership at all is a different sentence.
    assert_eq!(
        e.check(&human_claims(&ed), &other_space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::NoMembership
        }
    );
    // Suspending a principal who is not a member mints nothing.
    e.suspend(&space(), &user("stranger"));
    assert_eq!(e.membership_status(&space(), &user("stranger")), None);
}

#[test]
fn suspension_precedes_profile_resolution_entirely() {
    // No profile installed for the space at all: an active member would be
    // denied `NoProfileForSpace`. A suspended one is denied for being
    // suspended, because status is resolved first.
    let mut e = AccessEngine::new();
    let ed = user("e");
    e.set_membership(&space(), &ed, Role::Editor);
    assert_eq!(
        e.check(&human_claims(&ed), &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::NoProfileForSpace
        }
    );
    e.suspend(&space(), &ed);
    assert_eq!(
        e.check(&human_claims(&ed), &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::PrincipalSuspended
        }
    );
}

#[test]
fn a_group_grant_cannot_route_around_a_suspension() {
    let mut e = AccessEngine::new();
    let g = group("finance");
    e.set_profile(
        &space(),
        AccessProfile::new()
            .allow_group(&g, "txns.list")
            .group_holds_role(&g, Role::Owner)
            .allow(Role::Owner, "space.delete"),
    );
    let v = user("v");
    e.set_membership(&space(), &v, Role::Viewer);
    e.add_to_group(&space(), &v, &g);
    assert!(e
        .check(&human_claims(&v), &space(), "txns.list", T0)
        .is_allow());
    assert!(e
        .check(&human_claims(&v), &space(), "space.delete", T0)
        .is_allow());

    e.suspend(&space(), &v);
    for op in ["txns.list", "space.delete"] {
        assert_eq!(
            e.check(&human_claims(&v), &space(), op, T0),
            Decision::Deny {
                op: op.into(),
                reason: DenyReason::PrincipalSuspended
            },
            "neither the group allowlist nor the role it confers survives suspension"
        );
    }
}

#[test]
fn a_capability_token_cannot_route_around_a_suspension() {
    // The grant is valid, in the right space, unexpired, manifest-current,
    // and lists the op. The principal it acts as is suspended there.
    let (mut e, claims) = engine_with_grant(&["txns.list"]);
    assert!(e.check(&claims, &space(), "txns.list", T0).is_allow());

    e.set_membership(&space(), &claims.principal, Role::Editor);
    e.suspend(&space(), &claims.principal);
    assert_eq!(
        e.check(&claims, &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::PrincipalSuspended
        },
        "a capability grant is authority for a principal, and this principal is stopped"
    );
}

// ─────────────── groups and per-member overrides (SH-29) ────────────────

#[test]
fn group_allowlist_adds_to_the_role_allowlist() {
    let mut e = AccessEngine::new();
    let leads = group("leads");
    e.set_profile(
        &space(),
        AccessProfile::new()
            .allow(Role::Viewer, "txns.list")
            .allow_group(&leads, "txns.setCategory"),
    );
    let v = user("v");
    e.set_membership(&space(), &v, Role::Viewer);
    // Direct membership only: the role allowlist is the whole authority.
    assert!(e
        .check(&human_claims(&v), &space(), "txns.list", T0)
        .is_allow());
    assert!(!e
        .check(&human_claims(&v), &space(), "txns.setCategory", T0)
        .is_allow());
    // Joining the group adds the group's ops, and takes nothing away.
    e.add_to_group(&space(), &v, &leads);
    assert!(e
        .check(&human_claims(&v), &space(), "txns.setCategory", T0)
        .is_allow());
    assert!(e
        .check(&human_claims(&v), &space(), "txns.list", T0)
        .is_allow());
}

#[test]
fn group_conferred_role_combines_by_max_level() {
    // A group may hold a role. The caller's level is the max of the direct
    // role and every conferred one — and the ops of both are unioned, since
    // roles have no hierarchy.
    let mut e = AccessEngine::new();
    let managers = group("managers");
    e.set_profile(
        &space(),
        AccessProfile::new()
            .allow(Role::Viewer, "txns.list")
            .allow(Role::Manager, "space.invite")
            .group_holds_role(&managers, Role::Manager),
    );
    let v = user("v");
    e.set_membership(&space(), &v, Role::Viewer);
    e.add_to_group(&space(), &v, &managers);

    assert!(e
        .check(&human_claims(&v), &space(), "space.invite", T0)
        .is_allow());
    assert!(e
        .check(&human_claims(&v), &space(), "txns.list", T0)
        .is_allow());
    // A denial names the resolved level (manager), not the direct role.
    assert_eq!(
        e.check(&human_claims(&v), &space(), "space.delete", T0),
        Decision::Deny {
            op: "space.delete".into(),
            reason: DenyReason::OpNotInRole {
                role: Role::Manager
            },
        }
    );
}

#[test]
fn per_member_overrides_apply_last_and_deny_wins() {
    let mut e = AccessEngine::new();
    let leads = group("leads");
    let alice = user("alice");
    let bob = user("bob");
    e.set_profile(
        &space(),
        AccessProfile::new()
            .allow(Role::Editor, "txns.list")
            .allow_group(&leads, "space.delete")
            // Alice keeps the group grant but loses it by name…
            .override_deny(&alice, "space.delete")
            // …and Bob gets one op nothing else grants him.
            .override_allow(&bob, "reports.export"),
    );
    for u in [&alice, &bob] {
        e.set_membership(&space(), u, Role::Editor);
        e.add_to_group(&space(), u, &leads);
    }

    // Deny-by-override beats allow-by-group.
    assert!(!e
        .check(&human_claims(&alice), &space(), "space.delete", T0)
        .is_allow());
    // Bob, same role and same group, still holds it.
    assert!(e
        .check(&human_claims(&bob), &space(), "space.delete", T0)
        .is_allow());
    // Allow-by-override grants what neither role nor group did — and is
    // per member, so Alice does not inherit it.
    assert!(e
        .check(&human_claims(&bob), &space(), "reports.export", T0)
        .is_allow());
    assert!(!e
        .check(&human_claims(&alice), &space(), "reports.export", T0)
        .is_allow());
}

#[test]
fn effective_ops_resolves_all_three_sources() {
    // The resolution rule, read directly off the profile: union of role and
    // group allowlists, then overrides.
    let leads = group("leads");
    let alice = user("alice");
    let profile = AccessProfile::new()
        .allow(Role::Editor, "txns.list")
        .allow_group(&leads, "txns.setCategory")
        .allow_group(&leads, "space.delete")
        .override_allow(&alice, "reports.export")
        .override_deny(&alice, "space.delete");
    let groups: BTreeSet<GroupId> = [leads].into_iter().collect();
    assert_eq!(
        profile.effective_ops(&alice, Role::Editor, &groups),
        ops(&["txns.list", "txns.setCategory", "reports.export"])
    );
    // Group membership never leaks across principals.
    assert_eq!(
        profile.effective_ops(&user("carol"), Role::Editor, &BTreeSet::new()),
        ops(&["txns.list"])
    );
}

#[test]
fn a_group_cannot_smuggle_in_a_non_member() {
    // Groups are per space and presuppose membership: adding a stranger to a
    // group is not a membership-minting path.
    let mut e = engine_with_profile();
    let leads = group("leads");
    let stranger = user("s");
    e.add_to_group(&space(), &stranger, &leads);
    assert_eq!(
        e.check(&human_claims(&stranger), &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::NoMembership
        }
    );
}

// ─────────────────────── password provider round-trip ───────────────────

fn password_provider() -> LocalPasswordProvider<PlainTestHasher> {
    let p = LocalPasswordProvider::new(PlainTestHasher);
    p.enroll("alice", "correct horse").expect("enrolled");
    p
}

fn begin_challenge(p: &dyn AuthProvider) -> crate::provider::ChallengeId {
    match p
        .begin(
            &BeginRequest {
                identifier: Some("alice".into()),
                return_to: None,
                presence: false,
            },
            NOW,
        )
        .expect("begins")
    {
        ChallengeOrRedirect::Challenge { id, method } => {
            assert_eq!(method, AuthMethodKind::Password);
            id
        }
        other => panic!("expected challenge, got {other:?}"),
    }
}

/// Audit F7 (2026-10-24). Challenge ids were a counter, so anyone could
/// name the id their neighbour had just been handed and spend it with junk.
/// They are now 128 random bits, and a wrong id is the answer an expired one
/// is, and touches nobody else's challenge.
#[test]
fn password_challenge_ids_are_random_and_a_guess_burns_nobody() {
    let p = password_provider();
    let victim = begin_challenge(&p);
    let attacker = begin_challenge(&p);
    for id in [&victim.0, &attacker.0] {
        let hex = id.strip_prefix("pwd-ch-").expect("prefix");
        assert_eq!(hex.len(), 32, "128 bits: {id}");
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
    }
    assert_ne!(victim, attacker);
    let junk = |challenge: crate::provider::ChallengeId, at| {
        p.complete(
            &CompleteRequest::Password {
                challenge,
                identifier: "alice".into(),
                password: "x".into(),
            },
            at,
        )
    };
    // A guess at the victim's id: the neighbour of the attacker's, as the
    // counter would have made it.
    let guessed = crate::provider::ChallengeId("pwd-ch-00000000000000000000000000000001".into());
    let unknown = junk(guessed.clone(), NOW).expect_err("unknown");
    assert!(matches!(unknown, AuthError::UnknownChallenge(_)));
    // An expired challenge is the same answer.
    let stale = begin_challenge(&p);
    let expired = junk(stale, Timestamp(NOW.0 + 10_000)).expect_err("expired");
    assert_eq!(
        std::mem::discriminant(&expired),
        std::mem::discriminant(&unknown)
    );
    p.complete(
        &CompleteRequest::Password {
            challenge: victim,
            identifier: "alice".into(),
            password: "correct horse".into(),
        },
        NOW,
    )
    .expect("the victim's own challenge still works");
}

/// Audit F7. Anyone may begin, so the waiting challenges are capped, and a
/// begin past the cap names the setting that raises it.
#[test]
fn password_challenges_past_the_cap_are_refused_naming_the_setting() {
    let p = password_provider().with_pending_max(3);
    let ids: Vec<_> = (0..3).map(|_| begin_challenge(&p)).collect();
    let refused = p
        .begin(
            &BeginRequest {
                identifier: Some("alice".into()),
                return_to: None,
                presence: false,
            },
            NOW,
        )
        .expect_err("past the cap");
    assert_eq!(
        refused,
        AuthError::TooManyPending {
            setting: crate::provider::PASSWORD_PENDING_MAX_ENV,
            cap: 3
        }
    );
    assert!(refused
        .to_string()
        .contains("ORDO_AUTH_PASSWORD_PENDING_MAX"));
    // A completed challenge frees its place.
    p.complete(
        &CompleteRequest::Password {
            challenge: ids[0].clone(),
            identifier: "alice".into(),
            password: "correct horse".into(),
        },
        NOW,
    )
    .expect("verifies");
    begin_challenge(&p);
}

#[test]
fn password_round_trip() {
    let p = password_provider();
    let ch = begin_challenge(&p);
    let subject = p
        .complete(
            &CompleteRequest::Password {
                challenge: ch,
                identifier: "alice".into(),
                password: "correct horse".into(),
            },
            NOW,
        )
        .expect("good credentials verify");
    assert_eq!(
        subject.authenticator,
        AuthenticatorId("password:alice".into())
    );
    assert_eq!(subject.method, AuthMethodKind::Password);
}

#[test]
fn password_wrong_and_unknown_are_indistinguishable() {
    let p = password_provider();
    let ch1 = begin_challenge(&p);
    let wrong = p.complete(
        &CompleteRequest::Password {
            challenge: ch1,
            identifier: "alice".into(),
            password: "wrong".into(),
        },
        NOW,
    );
    let ch2 = begin_challenge(&p);
    let unknown = p.complete(
        &CompleteRequest::Password {
            challenge: ch2,
            identifier: "nobody".into(),
            password: "whatever".into(),
        },
        NOW,
    );
    assert_eq!(wrong, Err(AuthError::BadCredentials));
    assert_eq!(unknown, Err(AuthError::BadCredentials));
}

#[test]
fn password_challenge_is_one_shot() {
    let p = password_provider();
    let ch = begin_challenge(&p);
    let req = CompleteRequest::Password {
        challenge: ch.clone(),
        identifier: "alice".into(),
        password: "correct horse".into(),
    };
    assert!(p.complete(&req, NOW).is_ok());
    assert_eq!(p.complete(&req, NOW), Err(AuthError::UnknownChallenge(ch)));
}

#[test]
fn password_rejects_foreign_payload() {
    let p = password_provider();
    let res = p.complete(
        &CompleteRequest::OidcCallback {
            state: "st".into(),
            code: "c".into(),
            iss: None,
        },
        NOW,
    );
    assert_eq!(
        res,
        Err(AuthError::MethodMismatch {
            provider: AuthMethodKind::Password
        })
    );
}

// ───────────────────────── OIDC stub round-trip ─────────────────────────

fn oidc() -> StubOidcProvider {
    StubOidcProvider::new("google", "https://idp.example/authorize", "ordo-client")
}

fn begin_redirect(p: &StubOidcProvider) -> (String, String) {
    match p.begin(&BeginRequest::default(), NOW).expect("begins") {
        ChallengeOrRedirect::Redirect { url, .. } => {
            let state = url
                .split("state=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .expect("state in url")
                .to_string();
            (url, state)
        }
        other => panic!("expected redirect, got {other:?}"),
    }
}

fn callback(state: &str, code: &str) -> CompleteRequest {
    CompleteRequest::OidcCallback {
        state: state.into(),
        code: code.into(),
        iss: None,
    }
}

#[test]
fn oidc_round_trip() {
    let p = oidc();
    let (url, state) = begin_redirect(&p);
    assert!(url.contains(&format!("state={state}")));
    assert!(url.contains("client_id=ordo-client"));
    // The nonce rides the authorize URL; a real IdP echoes it in the
    // ID token. Extract it as the "IdP".
    let nonce = url
        .split("nonce=")
        .nth(1)
        .expect("nonce in url")
        .to_string();
    p.issue("authcode", "sub-123", &nonce);
    let subject = p
        .complete(&callback(&state, "authcode"), NOW)
        .expect("valid callback");
    assert_eq!(
        subject.authenticator,
        AuthenticatorId("oidc:google:sub-123".into())
    );
    assert_eq!(subject.method, AuthMethodKind::Oidc);
}

#[test]
fn oidc_unknown_state_rejected() {
    let p = oidc();
    let res = p.complete(&callback("forged", "c"), NOW);
    assert_eq!(res, Err(AuthError::StateMismatch("forged".into())));
}

#[test]
fn oidc_state_is_one_shot() {
    let p = oidc();
    let (url, state) = begin_redirect(&p);
    let nonce = url.split("nonce=").nth(1).unwrap().to_string();
    p.issue("c", "s", &nonce);
    let cb = callback(&state, "c");
    assert!(p.complete(&cb, NOW).is_ok());
    assert_eq!(p.complete(&cb, NOW), Err(AuthError::StateMismatch(state)));
}

#[test]
fn oidc_nonce_mismatch_rejected() {
    let p = oidc();
    let (_url, state) = begin_redirect(&p);
    // The IdP issued a token for another session's nonce.
    p.issue("c", "s", "replayed-nonce");
    let res = p.complete(&callback(&state, "c"), NOW);
    assert_eq!(res, Err(AuthError::NonceMismatch));
}

#[test]
fn oidc_empty_code_rejected() {
    let p = oidc();
    let (url, state) = begin_redirect(&p);
    let nonce = url.split("nonce=").nth(1).unwrap().to_string();
    p.issue("", "s", &nonce);
    let res = p.complete(&callback(&state, ""), NOW);
    assert_eq!(res, Err(AuthError::EmptyAuthorizationCode));
}

/// The callback carries no identity. A caller that knows a state and makes up
/// a code gets nothing: the subject is whatever the IdP issued for that code,
/// and it issued nothing.
#[test]
fn oidc_a_made_up_code_names_nobody() {
    let p = oidc();
    let (_url, state) = begin_redirect(&p);
    let res = p.complete(&callback(&state, "made-up"), NOW);
    assert_eq!(res, Err(AuthError::BadCredentials));
}

// ───────────────────────────── registry ─────────────────────────────────

#[test]
fn registry_routes_by_method_name() {
    let mut r = ProviderRegistry::new();
    r.register("password", Box::new(password_provider()))
        .unwrap();
    r.register("google", Box::new(oidc())).unwrap();

    assert_eq!(r.get("password").unwrap().kind(), AuthMethodKind::Password);
    assert_eq!(r.get("google").unwrap().kind(), AuthMethodKind::Oidc);
    assert_eq!(
        r.register("google", Box::new(oidc())).unwrap_err(),
        RegistryError::DuplicateMethod("google".into())
    );
    match r.get("discord") {
        Err(RegistryError::UnknownMethod(n)) => assert_eq!(n, "discord"),
        Err(other) => panic!("expected UnknownMethod, got {other:?}"),
        Ok(_) => panic!("expected UnknownMethod, got a provider"),
    }
    let methods: Vec<_> = r.methods().collect();
    assert_eq!(
        methods,
        vec![
            ("google", AuthMethodKind::Oidc),
            ("password", AuthMethodKind::Password)
        ]
    );
}

// ─────────────────────────── account linking ────────────────────────────

fn authr(s: &str) -> AuthenticatorId {
    AuthenticatorId(s.into())
}

/// A session that completed a presence ceremony at `at`, vouched for by an
/// authenticator the principal already holds.
fn ceremony(p: &PrincipalId, existing: &str, at: Timestamp) -> LinkCeremony {
    let claims = TokenClaims {
        user_presence_at: Some(at),
        ..human_claims(p)
    };
    LinkCeremony::from_claims(&claims, &authr(existing)).expect("a fresh human session")
}

#[test]
fn link_and_unlink_emit_audit_events() {
    let mut d = Directory::new(Provisioning::InviteOnly);
    let p = d
        .create_principal(PrincipalKind::Human, "Alice", T0)
        .unwrap();
    assert!(p.0.starts_with("usr_"));

    // The invitation being accepted: no factor exists yet to vouch.
    let linked = d.link_first(&p, &authr("password:alice"), T0).unwrap();
    assert_eq!(linked.kind, AuditEventKind::AuthenticatorLinked);
    assert_eq!(linked.principal, p);
    assert_eq!(linked.authenticator, Some(authr("password:alice")));
    assert_eq!(
        linked.existing_authenticator, None,
        "bootstrap has nothing to vouch"
    );
    assert_eq!(linked.at, T0);

    // Second authenticator — now a ceremony is required and possible.
    d.link(
        &p,
        &authr("oidc:google:sub-1"),
        &ceremony(&p, "password:alice", T0),
        T0,
    )
    .unwrap();
    let unlinked = d.unlink(&authr("password:alice"), T1).unwrap();
    assert_eq!(unlinked.kind, AuditEventKind::AuthenticatorUnlinked);
    assert_eq!(unlinked.principal, p);
    assert_eq!(unlinked.at, T1);
    assert_eq!(d.principal_for(&authr("password:alice")).unwrap(), None);
    assert_eq!(
        d.principal_for(&authr("oidc:google:sub-1")).unwrap(),
        Some(p.clone())
    );
}

#[test]
fn linking_is_never_silent_on_conflict() {
    let mut d = Directory::new(Provisioning::InviteOnly);
    let a = d.create_principal(PrincipalKind::Human, "A", T0).unwrap();
    let b = d.create_principal(PrincipalKind::Human, "B", T0).unwrap();
    d.link_first(&a, &authr("passkey:cred-1"), T0).unwrap();
    d.link_first(&b, &authr("passkey:cred-2"), T0).unwrap();
    assert_eq!(
        d.link(
            &b,
            &authr("passkey:cred-1"),
            &ceremony(&b, "passkey:cred-2", T0),
            T0
        ),
        Err(DirectoryError::AlreadyLinked {
            authenticator: authr("passkey:cred-1"),
            to: a
        })
    );
}

#[test]
fn last_human_authenticator_cannot_be_unlinked() {
    let mut d = Directory::new(Provisioning::InviteOnly);
    let p = d
        .create_principal(PrincipalKind::Human, "Alice", T0)
        .unwrap();
    d.link_first(&p, &authr("password:alice"), T0).unwrap();
    assert_eq!(
        d.unlink(&authr("password:alice"), T1),
        Err(DirectoryError::LastAuthenticator { principal: p })
    );
}

// ─── Z-006: a stolen session must not mint a permanent credential ────────

#[test]
fn linking_from_an_ordinary_session_without_presence_is_refused() {
    // The attack in one line: a stolen bearer token is an authenticated
    // session with no fresh presence, and it must not be able to attach a
    // factor that outlives the theft.
    let p = user("alice");
    let stolen = human_claims(&p); // authenticated, no presence assertion
    assert_eq!(
        LinkCeremony::from_claims(&stolen, &authr("password:alice")),
        Err(DirectoryError::StepUpRequired { principal: p }),
        "no ceremony can be built, so `link` cannot even be called"
    );
}

#[test]
fn linking_with_a_stale_presence_assertion_is_refused() {
    let mut d = Directory::new(Provisioning::InviteOnly).with_link_presence_max_age(60);
    let p = d
        .create_principal(PrincipalKind::Human, "Alice", T0)
        .unwrap();
    d.link_first(&p, &authr("password:alice"), T0).unwrap();
    let at = Timestamp(10_000);
    let proof = ceremony(&p, "password:alice", at);

    // Inside the window, boundary instant included.
    assert!(d
        .link(&p, &authr("passkey:new-1"), &proof, Timestamp(at.0 + 60))
        .is_ok());
    // One second past it: presence gated the login, not this ceremony.
    assert_eq!(
        d.link(&p, &authr("passkey:new-2"), &proof, Timestamp(at.0 + 61)),
        Err(DirectoryError::StepUpStale {
            age_secs: 61,
            max_age_secs: 60
        })
    );
    // A presence stamped in the future is a clock lie, not freshness.
    assert!(matches!(
        d.link(&p, &authr("passkey:new-3"), &proof, Timestamp(at.0 - 1)),
        Err(DirectoryError::StepUpStale { .. })
    ));
}

#[test]
fn a_capability_token_can_never_link() {
    let mut e = AccessEngine::new();
    e.deploy_op("txns.list", base_pin(b"v1"));
    let grant = e
        .grant_for(GrantId("gr_test".into()), &space(), ops(&["txns.list"]), T1)
        .unwrap();
    let claims = TokenClaims {
        principal: PrincipalId("agt_a".into()),
        capability: Some(grant),
        client: None,
        // Even a token that *claims* fresh presence: the refusal is
        // structural, not a convention about what minters put in the claim.
        user_presence_at: Some(T0),
        delegation: None,
        session: None,
    };
    assert_eq!(
        LinkCeremony::from_claims(&claims, &authr("passkey:cred-1")),
        Err(DirectoryError::CapabilityCannotLink {
            principal: PrincipalId("agt_a".into())
        })
    );
}

#[test]
fn a_successful_link_names_both_authenticators_in_its_audit_event() {
    let mut d = Directory::new(Provisioning::InviteOnly);
    let p = d
        .create_principal(PrincipalKind::Human, "Alice", T0)
        .unwrap();
    d.link_first(&p, &authr("password:alice"), T0).unwrap();

    let event = d
        .link(
            &p,
            &authr("passkey:cred-9"),
            &ceremony(&p, "password:alice", T1),
            T1,
        )
        .expect("fresh presence on a linked factor authorizes the link");
    assert_eq!(event.kind, AuditEventKind::AuthenticatorLinked);
    assert_eq!(event.principal, p);
    assert_eq!(
        event.authenticator,
        Some(authr("passkey:cred-9")),
        "the factor added"
    );
    assert_eq!(
        event.existing_authenticator,
        Some(authr("password:alice")),
        "the factor that authorized it — the question asked after a takeover"
    );
    assert_eq!(event.at, T1);
}

#[test]
fn the_vouching_authenticator_must_belong_to_the_principal() {
    let mut d = Directory::new(Provisioning::InviteOnly);
    let a = d.create_principal(PrincipalKind::Human, "A", T0).unwrap();
    let b = d.create_principal(PrincipalKind::Human, "B", T0).unwrap();
    d.link_first(&a, &authr("passkey:a-1"), T0).unwrap();
    d.link_first(&b, &authr("passkey:b-1"), T0).unwrap();

    // Fresh presence on *B*'s factor does not authorize a link onto A.
    assert_eq!(
        d.link(
            &a,
            &authr("passkey:attacker"),
            &ceremony(&a, "passkey:b-1", T0),
            T0
        ),
        Err(DirectoryError::ExistingAuthenticatorNotLinked {
            authenticator: authr("passkey:b-1"),
            principal: a.clone(),
        })
    );
    // …and a ceremony proving presence for B cannot be aimed at A either.
    assert_eq!(
        d.link(
            &a,
            &authr("passkey:attacker"),
            &ceremony(&b, "passkey:b-1", T0),
            T0
        ),
        Err(DirectoryError::CeremonyPrincipalMismatch {
            ceremony: b,
            requested: a
        })
    );
}

#[test]
fn the_bootstrap_path_closes_once_a_factor_exists() {
    // `link_first` is the invitation/onboard path. If it stayed open it would
    // be the takeover path with extra steps, so it refuses any account that
    // already holds a factor — which every logged-in account does.
    let mut d = Directory::new(Provisioning::InviteOnly);
    let p = d
        .create_principal(PrincipalKind::Human, "Alice", T0)
        .unwrap();
    d.link_first(&p, &authr("password:alice"), T0).unwrap();
    assert_eq!(
        d.link_first(&p, &authr("passkey:attacker"), T1),
        Err(DirectoryError::StepUpRequired { principal: p })
    );
}

#[test]
fn invite_only_rejects_unknown_authenticator() {
    let mut d = Directory::new(Provisioning::InviteOnly);
    let subject = crate::provider::AuthenticatedSubject::without_presence(
        authr("oidc:google:new-face"),
        AuthMethodKind::Oidc,
    );
    assert_eq!(
        d.resolve_or_onboard(&subject, "New Face", T0),
        Err(DirectoryError::NotInvited(authr("oidc:google:new-face")))
    );
}

/// 25ar: under `InviteOnly`, a caller that spent an invitation for this
/// person has them minted, and a known authenticator still resolves to its
/// own principal whatever the caller says.
#[test]
fn invite_only_mints_an_invited_newcomer_and_nobody_else() {
    use crate::directory::Onboarding;
    let mut d = Directory::new(Provisioning::InviteOnly);
    let subject = crate::provider::AuthenticatedSubject::without_presence(
        authr("oidc:google:invited"),
        AuthMethodKind::Oidc,
    );
    let (p, events) = d
        .resolve_or_onboard_as(&subject, "Invited", T0, Onboarding::Invited)
        .unwrap();
    assert_eq!(events.len(), 2, "onboarded and linked, both audited");
    let (again, none) = d.resolve_or_onboard(&subject, "Invited", T1).unwrap();
    assert_eq!((again, none.len()), (p, 0));

    let stranger = crate::provider::AuthenticatedSubject::without_presence(
        authr("oidc:google:stranger"),
        AuthMethodKind::Oidc,
    );
    assert_eq!(
        d.resolve_or_onboard_as(&stranger, "S", T1, Onboarding::ByPolicy),
        Err(DirectoryError::NotInvited(authr("oidc:google:stranger")))
    );
}

#[test]
fn auto_onboard_mints_and_links_with_audit_trail() {
    let mut d = Directory::new(Provisioning::AutoOnboard);
    let subject = crate::provider::AuthenticatedSubject::without_presence(
        authr("oidc:google:new-face"),
        AuthMethodKind::Oidc,
    );
    let (p, events) = d.resolve_or_onboard(&subject, "New Face", T0).unwrap();
    assert_eq!(d.principal(&p).unwrap().unwrap().kind, PrincipalKind::Human);
    let kinds: Vec<_> = events.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            AuditEventKind::PrincipalOnboarded,
            AuditEventKind::AuthenticatorLinked
        ]
    );

    // Second login: resolves, no new events.
    let (again, events2) = d.resolve_or_onboard(&subject, "New Face", T1).unwrap();
    assert_eq!(again, p);
    assert!(events2.is_empty());
}

#[test]
fn a_colliding_draw_is_retried_and_never_minted() {
    // The store's id source is rigged to draw the same suffix twice, so the
    // first `create_principal` commits it and the second must find it
    // already taken and draw again — proving the retry runs rather than
    // trusting the odds a 128-bit draw never collides in practice.
    let draws = std::sync::Mutex::new(vec!["second", "dup", "dup"]);
    let store: std::sync::Arc<dyn DirectoryStore> = std::sync::Arc::new(
        MemoryDirectoryStore::new().with_id_source(Box::new(move || {
            draws
                .lock()
                .unwrap()
                .pop()
                .expect("no more rigged draws")
                .to_string()
        })),
    );
    let first = store
        .create_principal(PrincipalKind::Human, "A", T0)
        .unwrap();
    assert_eq!(first.0, "usr_dup");
    // Second call draws "dup" again (collision with `first`), then "second".
    let second = store
        .create_principal(PrincipalKind::Human, "B", T0)
        .unwrap();
    assert_eq!(second.0, "usr_second");
    assert_ne!(first, second, "a collision must never be minted");
}

#[test]
fn a_source_that_only_ever_collides_refuses_rather_than_spinning() {
    let store = MemoryDirectoryStore::new().with_id_source(Box::new(|| "stuck".to_string()));
    store
        .create_principal(PrincipalKind::Human, "A", T0)
        .unwrap();
    // Every subsequent draw collides with the row above; the bound must
    // stop the loop rather than spin forever.
    let err = store.create_principal(PrincipalKind::Human, "B", T0);
    assert!(err.is_err());
}

// ───────────────── callersOf: who can call this, and how (8r) ────────────
//
// `security-logging-design.md` §6 rule 4, in five tests. The rule's own
// standard is EXHAUSTIVE — *"a group grant that does not show up there
// silently breaks the model's best property"* — so each test is written
// against an omission rather than an inclusion: the assertions that matter
// are the ones that would fail if a via went missing.

fn caller_pairs(cs: &[crate::access::Caller]) -> Vec<(String, String, String)> {
    cs.iter()
        .map(|c| {
            (
                c.principal.0.clone(),
                c.via.kind().to_string(),
                c.via.detail(),
            )
        })
        .collect()
}

/// All four vias, in one space, for one op — the whole of rule 4.
#[test]
fn callers_of_names_role_group_override_and_token() {
    use crate::access::Caller;
    use crate::grants::GrantRecord;

    let mut e = AccessEngine::new();
    let profile = AccessProfile::new()
        .allow(Role::Manager, "closeTask")
        .allow_group(&group("ops"), "closeTask")
        .group_holds_role(&group("leads"), Role::Manager)
        .override_allow(&user("guest"), "closeTask");
    e.set_profile(&space(), profile);

    e.set_membership(&space(), &user("mgr"), Role::Manager);
    e.set_membership(&space(), &user("in_ops"), Role::Viewer);
    e.add_to_group(&space(), &user("in_ops"), &group("ops"));
    e.set_membership(&space(), &user("lead"), Role::Viewer);
    e.add_to_group(&space(), &user("lead"), &group("leads"));
    e.set_membership(&space(), &user("guest"), Role::Viewer);
    // A principal with no path at all — the control. Rule 4 is about not
    // omitting; a surface that listed everyone would satisfy every
    // inclusion assertion below and be worthless.
    e.set_membership(&space(), &user("nobody"), Role::Viewer);

    let record = GrantRecord {
        id: GrantId("gr_1".into()),
        space: space(),
        principal: user("app_owner"),
        client: Some(ClientId("cli_dash".into())),
        op: "closeTask".into(),
        manifest_hash: ManifestHash([1; 32]),
        pin: String::new(),
        granted_at: Timestamp(0),
        expires: Timestamp(99_999),
        revoked_at: None,
    };

    let found = caller_pairs(&e.callers_of(&space(), "closeTask", &[record], &[], NOW));
    assert!(
        found.contains(&("usr_mgr".into(), "role".into(), "manager".into())),
        "{found:?}"
    );
    assert!(
        found.contains(&("usr_in_ops".into(), "group".into(), "grp_ops".into())),
        "a group's own allowlist is a via: {found:?}"
    );
    assert!(
        found.contains(&(
            "usr_lead".into(),
            "group".into(),
            "grp_leads (confers manager)".into()
        )),
        "a group CONFERRING a role is also a group via, and names the role \
         — otherwise a manager cannot tell which edit removes it: {found:?}"
    );
    assert!(
        found.contains(&("usr_guest".into(), "override".into(), String::new())),
        "{found:?}"
    );
    assert!(
        found.contains(&(
            "usr_app_owner".into(),
            "token".into(),
            "gr_1 (client cli_dash)".into()
        )),
        "the token column — the one 8r was blocked on: {found:?}"
    );
    assert!(
        !found.iter().any(|(p, _, _)| p == "usr_nobody"),
        "a member with no path was listed: {found:?}"
    );

    // And every listed caller really can call it. The two surfaces are
    // computed differently — this walks the model, `check` decides one
    // principal — so agreeing is evidence rather than a tautology.
    for Caller { principal, via } in e.callers_of(&space(), "closeTask", &[], &[], NOW) {
        assert_eq!(
            e.check(&human_claims(&principal), &space(), "closeTask", NOW),
            Decision::Allow,
            "{principal:?} listed via {via:?} is denied at the door"
        );
    }
}

/// A principal reached three ways is three rows. Asserted because the
/// tempting implementation — a set of principals — passes every
/// "is X listed" test and answers the question a manager actually has
/// (*"how do I stop them"*) wrongly.
#[test]
fn every_via_is_reported_not_just_the_first() {
    let mut e = AccessEngine::new();
    e.set_profile(
        &space(),
        AccessProfile::new()
            .allow(Role::Manager, "closeTask")
            .allow_group(&group("ops"), "closeTask")
            .override_allow(&user("busy"), "closeTask"),
    );
    e.set_membership(&space(), &user("busy"), Role::Manager);
    e.add_to_group(&space(), &user("busy"), &group("ops"));

    let vias: Vec<String> = e
        .callers_of(&space(), "closeTask", &[], &[], NOW)
        .iter()
        .map(|c| c.via.kind().to_string())
        .collect();
    assert_eq!(vias, vec!["role", "group", "override"]);
}

/// The subtractions, each of which a union-shaped implementation gets wrong.
/// Every one of them is a caller the door refuses, so listing them would make
/// the audit surface disagree with the door in the dangerous direction:
/// *"this is who can call it"* naming someone who cannot.
#[test]
fn a_deny_override_a_suspension_and_a_dead_grant_each_remove_a_caller() {
    use crate::grants::GrantRecord;

    let mut e = AccessEngine::new();
    e.set_profile(
        &space(),
        AccessProfile::new()
            .allow(Role::Manager, "closeTask")
            .allow_group(&group("ops"), "closeTask")
            // Deny beats allow-by-group AND allow-by-role, applied last.
            .override_deny(&user("denied"), "closeTask"),
    );
    e.set_membership(&space(), &user("denied"), Role::Manager);
    e.add_to_group(&space(), &user("denied"), &group("ops"));
    e.set_membership(&space(), &user("suspended"), Role::Manager);
    e.set_membership(&space(), &user("token_holder"), Role::Viewer);
    e.set_membership(&space(), &user("live"), Role::Manager);

    let grant = |principal: &str, revoked: Option<Timestamp>, expires: Timestamp| GrantRecord {
        id: GrantId(format!("gr_{principal}")),
        space: space(),
        principal: user(principal),
        client: None,
        op: "closeTask".into(),
        manifest_hash: ManifestHash([1; 32]),
        pin: String::new(),
        granted_at: Timestamp(0),
        expires,
        revoked_at: revoked,
    };

    // The deny-override is already in force, and it is the subtraction that
    // is easiest to get wrong: `usr_denied` is reached by BOTH their role and
    // their group, and a union-shaped implementation would list them twice.
    // Three rows — the suspended manager (still active here), the live
    // manager, and the token — and none of them is `usr_denied`.
    let start = caller_pairs(&e.callers_of(
        &space(),
        "closeTask",
        &[grant("token_holder", None, Timestamp(99_999))],
        &[],
        NOW,
    ));
    assert_eq!(start.len(), 3, "{start:?}");
    assert!(
        !start.iter().any(|(p, _, _)| p == "usr_denied"),
        "a per-member deny must subtract every via at once, not one of \
         them: {start:?}"
    );

    // Now suspend, and present a revoked grant and an expired one.
    e.suspend(&space(), &user("suspended"));
    let records = [
        grant("token_holder", Some(Timestamp(5_000)), Timestamp(9_999)),
        grant("live", None, Timestamp(1)),
    ];
    let found = caller_pairs(&e.callers_of(&space(), "closeTask", &records, &[], NOW));
    assert_eq!(
        found,
        vec![(
            "usr_live".to_string(),
            "role".to_string(),
            "manager".to_string()
        )],
        "deny-override, suspension, revocation and expiry must each remove a \
         caller the door would refuse: {found:?}"
    );

    // Suspension must also kill a LIVE token, because the door checks status
    // before it looks at the grant and the registry cannot know that.
    let found = caller_pairs(&e.callers_of(
        &space(),
        "closeTask",
        &[grant("suspended", None, Timestamp(99_999))],
        &[],
        NOW,
    ));
    assert!(
        !found.iter().any(|(p, _, _)| p == "usr_suspended"),
        "a suspended principal's live token was listed: {found:?}"
    );
}

/// Space isolation, in both directions: the walk never leaves the space, and
/// a grant record from elsewhere is refused even when handed in.
///
/// The second half matters because the records arrive as an argument — the
/// engine cannot assume its caller queried the right space, and an audit
/// surface that trusted the argument would report another space's tokens as
/// this one's.
#[test]
fn callers_of_never_crosses_a_space_boundary() {
    use crate::grants::GrantRecord;

    let mut e = AccessEngine::new();
    let profile = AccessProfile::new().allow(Role::Manager, "closeTask");
    e.set_profile(&space(), profile.clone());
    e.set_profile(&other_space(), profile);
    e.set_membership(&space(), &user("here"), Role::Manager);
    e.set_membership(&other_space(), &user("there"), Role::Manager);

    let elsewhere = GrantRecord {
        id: GrantId("gr_elsewhere".into()),
        space: other_space(),
        principal: user("app"),
        client: None,
        op: "closeTask".into(),
        manifest_hash: ManifestHash([1; 32]),
        pin: String::new(),
        granted_at: Timestamp(0),
        expires: Timestamp(99_999),
        revoked_at: None,
    };

    let found = caller_pairs(&e.callers_of(&space(), "closeTask", &[elsewhere], &[], NOW));
    assert_eq!(
        found,
        vec![(
            "usr_here".to_string(),
            "role".to_string(),
            "manager".to_string()
        )]
    );
    // And an op nobody reaches answers empty rather than answering about
    // some other op.
    assert!(e.callers_of(&space(), "noSuchOp", &[], &[], NOW).is_empty());
}

/// `membership_reaches` and `callers_of` are one predicate. The surface that
/// gates the audit read uses the same function the audit read reports with,
/// so the gate and the answer cannot come to disagree about who is included.
///
/// Asserted over a cross product rather than an example: the failure this
/// prevents is drift between two functions, and one example would not see it.
#[test]
fn membership_reaches_agrees_with_callers_of_for_every_member() {
    let mut e = AccessEngine::new();
    e.set_profile(
        &space(),
        AccessProfile::new()
            .allow(Role::Manager, "closeTask")
            .allow(Role::Viewer, "taskBoard")
            .allow_group(&group("ops"), "closeTask")
            .override_deny(&user("d"), "closeTask")
            .override_allow(&user("g"), "closeTask"),
    );
    for (u, role) in [
        ("m", Role::Manager),
        ("v", Role::Viewer),
        ("d", Role::Manager),
        ("g", Role::Viewer),
        ("s", Role::Manager),
        ("o", Role::Viewer),
    ] {
        e.set_membership(&space(), &user(u), role);
    }
    e.add_to_group(&space(), &user("o"), &group("ops"));
    e.suspend(&space(), &user("s"));

    for op in ["closeTask", "taskBoard", "noSuchOp"] {
        let listed: BTreeSet<String> = e
            .callers_of(&space(), op, &[], &[], NOW)
            .iter()
            .map(|c| c.principal.0.clone())
            .collect();
        for u in ["m", "v", "d", "g", "s", "o"] {
            assert_eq!(
                listed.contains(&format!("usr_{u}")),
                e.membership_reaches(&space(), &user(u), op),
                "`{u}` disagrees between callers_of and membership_reaches on `{op}`"
            );
        }
    }
}

/// G5: `callable_by` is `callers_of` read the other way. For every member
/// and every op, the vias one names for the pair are the vias the other
/// names, so "who can call X" and "what can Y call" never disagree.
#[test]
fn callable_by_agrees_with_callers_of_for_every_member_and_op() {
    let mut e = AccessEngine::new();
    e.set_profile(
        &space(),
        AccessProfile::new()
            .allow(Role::Manager, "closeTask")
            .allow(Role::Viewer, "taskBoard")
            .allow_group(&group("ops"), "closeTask")
            .override_deny(&user("d"), "closeTask")
            .override_allow(&user("g"), "closeTask"),
    );
    for (u, role) in [
        ("m", Role::Manager),
        ("v", Role::Viewer),
        ("d", Role::Manager),
        ("g", Role::Viewer),
        ("s", Role::Manager),
        ("o", Role::Viewer),
    ] {
        e.set_membership(&space(), &user(u), role);
    }
    e.add_to_group(&space(), &user("o"), &group("ops"));
    e.suspend(&space(), &user("s"));
    let ops = ["closeTask", "taskBoard", "noSuchOp"];
    for u in ["m", "v", "d", "g", "s", "o", "nobody"] {
        let mine: BTreeSet<(String, String)> = e
            .callable_by(&space(), &user(u), ops, &[], &[], NOW)
            .into_iter()
            .map(|(op, via)| (op, via.detail()))
            .collect();
        let theirs: BTreeSet<(String, String)> = ops
            .iter()
            .flat_map(|op| {
                e.callers_of(&space(), op, &[], &[], NOW)
                    .into_iter()
                    .filter(|c| c.principal == user(u))
                    .map(|c| (op.to_string(), c.via.detail()))
            })
            .collect();
        assert_eq!(mine, theirs, "`{u}`");
    }
    // The suspended manager and the denied one reach nothing by role.
    assert!(e
        .callable_by(&space(), &user("s"), ops, &[], &[], NOW)
        .is_empty());
    assert!(e
        .callable_by(&space(), &user("o"), ops, &[], &[], NOW)
        .iter()
        .any(|(op, via)| op == "closeTask" && via.kind() == "group"));
}

// ───────────── delegation: acting as somebody, with their consent ────────
//
// `definition-language.md` §8 [settled]: *"delegation is itself a grant —
// acting on behalf of a user requires that user's own consent (user →
// agent, expiring, revocable, command-logged), distinct from the capability
// token's op allowlist. A manager granting an agent ops never grants it
// someone else's face."*
//
// Everything below is the half decidable **from the token**. Revocation is
// the door's registry gate and is proved against real Postgres in
// `ordo-host/tests/delegation.rs`, because a bearer string cannot express
// "withdrawn" and pretending otherwise here would prove nothing.

use crate::delegation::{DelegationClaim, DelegationRecord};
use crate::principal::DelegationId;

/// `usr_ada` delegates to `agt_ellanor` in `space()`, expiring at `at`.
fn ada_delegation(at: Timestamp) -> DelegationClaim {
    DelegationClaim {
        id: DelegationId("dlg_1".into()),
        space: space(),
        delegator: user("ada"),
        expires: at,
    }
}

/// The two consents a delegated agent needs, both present: a capability
/// grant listing the ops, and Ada's delegation. Ada is an editor, so she
/// reaches `txns.setCategory` and not `space.delete`.
fn delegated(
    op_allowlist: &[&str],
    delegation: Option<DelegationClaim>,
) -> (AccessEngine, TokenClaims) {
    let mut e = engine_with_profile();
    for op in ["txns.list", "txns.setCategory", "space.delete"] {
        e.deploy_op(op, base_pin(op.as_bytes()));
    }
    e.set_membership(&space(), &user("ada"), Role::Editor);
    let grant = e
        .grant_for(GrantId("gr_test".into()), &space(), ops(op_allowlist), T1)
        .expect("all ops deployed");
    let claims = TokenClaims {
        principal: PrincipalId("agt_ellanor".into()),
        capability: Some(grant),
        client: None,
        user_presence_at: None,
        delegation,
        session: None,
    };
    (e, claims)
}

#[test]
fn a_delegation_from_another_space_is_not_currency_here() {
    // SH-20's pin, at the door: delegation is per-space, never
    // per-principal-globally. An agent Ada delegated in her family space
    // must not be Ada at work just because it named the work space's id.
    let elsewhere = DelegationClaim {
        space: other_space(),
        ..ada_delegation(T1)
    };
    let (e, claims) = delegated(&["txns.setCategory"], Some(elsewhere));
    assert_eq!(
        e.check(&claims, &space(), "txns.setCategory", T0),
        Decision::Deny {
            op: "txns.setCategory".into(),
            reason: DenyReason::DelegationSpaceMismatch
        }
    );
}

#[test]
fn an_expired_delegation_is_refused_without_asking_anybody() {
    // Decided from the token, so a lapsed delegation stops working whether
    // or not the registry is reachable — the same reason a grant carries
    // its own expiry.
    let (e, claims) = delegated(&["txns.setCategory"], Some(ada_delegation(T0)));
    assert_eq!(
        e.check(&claims, &space(), "txns.setCategory", T0),
        Decision::Deny {
            op: "txns.setCategory".into(),
            reason: DenyReason::DelegationExpired
        },
        "the expiry instant itself denies, as it does for grants and tokens"
    );
    // …and one second earlier it does not.
    assert!(e
        .check(&claims, &space(), "txns.setCategory", Timestamp(T0.0 - 1))
        .is_allow());
}

#[test]
fn a_delegation_carries_only_the_delegators_authority() {
    // The escalation this rule exists to stop: whoever administers the
    // agent puts `space.delete` on its allowlist, Ada consents to "act as
    // me", and the agent reaches something Ada — an editor — never could.
    // The two consents INTERSECT.
    let (e, claims) = delegated(
        &["txns.setCategory", "space.delete"],
        Some(ada_delegation(T1)),
    );
    assert_eq!(
        e.check(&claims, &space(), "space.delete", T0),
        Decision::Deny {
            op: "space.delete".into(),
            reason: DenyReason::DelegationBeyondDelegator
        }
    );
    // The op Ada does reach is served — the bound is a bound, not a ban.
    assert!(e
        .check(&claims, &space(), "txns.setCategory", T0)
        .is_allow());

    // And the same agent, with the same allowlist and NO delegation, is
    // back to its own authority: §8's "without a live delegation grant the
    // agent acts only as itself". `space.delete` is on its allowlist, so it
    // passes — which is exactly why the delegation must not add to it.
    let (e2, claims2) = delegated(&["space.delete"], None);
    assert!(e2.check(&claims2, &space(), "space.delete", T0).is_allow());
}

#[test]
fn suspending_the_delegator_stops_the_delegation_at_the_next_call() {
    let (mut e, claims) = delegated(&["txns.setCategory"], Some(ada_delegation(T1)));
    assert!(e
        .check(&claims, &space(), "txns.setCategory", T0)
        .is_allow());
    // Suspension is how an operator stops a compromised member NOW (Z-005).
    // A delegation that outlived it would be the suspension's one loophole:
    // the person is stopped, the thing wearing their face is not.
    e.suspend(&space(), &user("ada"));
    assert_eq!(
        e.check(&claims, &space(), "txns.setCategory", T0),
        Decision::Deny {
            op: "txns.setCategory".into(),
            reason: DenyReason::DelegationBeyondDelegator
        }
    );
}

#[test]
fn an_op_closed_to_delegation_is_refused_to_a_delegated_token_and_to_nobody_else() {
    // SH-20: some ops are closed to every delegated token, whoever the
    // delegator is and whatever the agent's allowlist names. The engine
    // takes the list; the deployment decides what is on it.
    let (mut e, claims) = delegated(&["txns.setCategory"], Some(ada_delegation(T1)));
    assert!(e.delegable("txns.setCategory"));
    e.forbid_delegation("txns.setCategory");
    assert!(!e.delegable("txns.setCategory"));
    assert_eq!(
        e.check(&claims, &space(), "txns.setCategory", T0),
        Decision::Deny {
            op: "txns.setCategory".into(),
            reason: DenyReason::OpNotDelegable
        },
        "named as closed to delegation, not blamed on Ada's reach"
    );

    // Ada herself still may: the closure is about who acts, not the op.
    let ada = TokenClaims {
        principal: user("ada"),
        capability: None,
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };
    assert!(e.check(&ada, &space(), "txns.setCategory", T0).is_allow());

    // And so does the same agent acting as itself, with no delegation.
    let itself = TokenClaims {
        delegation: None,
        ..claims
    };
    assert!(e
        .check(&itself, &space(), "txns.setCategory", T0)
        .is_allow());
}

#[test]
fn the_allowlist_still_binds_a_delegated_agent() {
    // The other direction of "distinct from the token's op allowlist": Ada
    // reaches `txns.list`, and the agent's allowlist does not carry it. Her
    // consent to be acted for is not a consent to every op she holds.
    let (e, claims) = delegated(&["txns.setCategory"], Some(ada_delegation(T1)));
    assert_eq!(
        e.check(&claims, &space(), "txns.list", T0),
        Decision::Deny {
            op: "txns.list".into(),
            reason: DenyReason::OpNotInAllowlist
        }
    );
}

#[test]
fn a_delegated_token_still_cannot_satisfy_step_up() {
    // Presence is not delegable (§5), and a delegation is the one thing
    // that most looks like it should be an exception. It is not: the
    // structural refusal is on the token's *shape*, so nothing a minter
    // writes into a claim can talk its way past it.
    let (mut e, mut claims) = delegated(&["txns.setCategory"], Some(ada_delegation(T1)));
    e.require_step_up("txns.setCategory");
    claims.user_presence_at = Some(T0);
    assert_eq!(
        e.check(&claims, &space(), "txns.setCategory", T0),
        Decision::Deny {
            op: "txns.setCategory".into(),
            reason: DenyReason::StepUpRequired
        }
    );
}

#[test]
fn a_delegation_claim_survives_the_token_round_trip() {
    use crate::token::{mint, verify, TokenKey, TokenSpec};
    let key = TokenKey::from_secret(b"delegation-secret");
    let spec = TokenSpec {
        principal: PrincipalId("agt_ellanor".into()),
        expires: Timestamp(5_000),
        client: None,
        user_presence_at: None,
        capability: None,
        delegation: Some(ada_delegation(T1)),
        session: None,
    };
    let claims = verify(&key, &mint(&key, &spec), T0).expect("valid");
    assert_eq!(claims.delegation, Some(ada_delegation(T1)));

    // A token that claims no delegation round-trips as claiming none — the
    // absent case has to be unambiguous, or "acting as nobody" and "acting
    // as somebody the parser dropped" would be one value.
    let plain = TokenSpec {
        delegation: None,
        ..spec
    };
    assert_eq!(
        verify(&key, &mint(&key, &plain), T0)
            .expect("valid")
            .delegation,
        None
    );
}

/// One live delegation record, from Ada to `agent`.
fn delegation_record(agent: &str) -> DelegationRecord {
    DelegationRecord {
        id: DelegationId("dlg_1".into()),
        space: space(),
        delegator: user("ada"),
        agent: PrincipalId(agent.into()),
        granted_at: Timestamp(0),
        expires: Timestamp(99_999),
        revoked_at: None,
        consent_correlation: "cg_consent".into(),
    }
}

fn token_record(principal: &str) -> crate::grants::GrantRecord {
    crate::grants::GrantRecord {
        id: GrantId("gr_1".into()),
        space: space(),
        principal: PrincipalId(principal.into()),
        client: None,
        op: "closeTask".into(),
        manifest_hash: ManifestHash([1; 32]),
        pin: String::new(),
        granted_at: Timestamp(0),
        expires: Timestamp(99_999),
        revoked_at: None,
    }
}

/// `callersOf`'s fifth via, and the conjunction that makes it honest.
#[test]
fn callers_of_names_the_delegation_and_only_when_both_consents_hold() {
    let mut e = AccessEngine::new();
    e.set_profile(
        &space(),
        AccessProfile::new().allow(Role::Manager, "closeTask"),
    );
    e.set_membership(&space(), &user("ada"), Role::Manager);
    e.set_membership(&space(), &PrincipalId("agt_ellanor".into()), Role::Viewer);

    let token = token_record("agt_ellanor");
    let dlg = delegation_record("agt_ellanor");

    // Both consents: two rows for one agent, because there are two ways to
    // stop it and they are different people's to perform.
    let both = caller_pairs(&e.callers_of(
        &space(),
        "closeTask",
        std::slice::from_ref(&token),
        std::slice::from_ref(&dlg),
        NOW,
    ));
    assert!(
        both.contains(&(
            "agt_ellanor".into(),
            "delegation".into(),
            "on behalf of usr_ada (delegation dlg_1)".into()
        )),
        "the delegation path is its own via: {both:?}"
    );
    assert!(
        both.iter()
            .any(|(p, k, _)| p == "agt_ellanor" && k == "token"),
        "…and the token path is still there — revoking one does not stop \
         the other: {both:?}"
    );

    // Delegation alone, no token: the door would answer `OpNotInAllowlist`,
    // so listing it would name a caller that cannot call.
    let no_token =
        caller_pairs(&e.callers_of(&space(), "closeTask", &[], std::slice::from_ref(&dlg), NOW));
    assert!(
        !no_token.iter().any(|(p, _, _)| p == "agt_ellanor"),
        "a delegation with no capability grant reaches nothing: {no_token:?}"
    );

    // A revoked delegation is gone from the answer even if the caller hands
    // it over — the surface applies `is_live` itself rather than trusting
    // that the right rows arrived.
    let revoked = DelegationRecord {
        revoked_at: Some(Timestamp(1)),
        ..dlg.clone()
    };
    let after = caller_pairs(&e.callers_of(
        &space(),
        "closeTask",
        std::slice::from_ref(&token),
        &[revoked],
        NOW,
    ));
    assert!(
        !after.iter().any(|(_, k, _)| k == "delegation"),
        "{after:?}"
    );

    // Ada loses the op: the delegation conveys nothing, and the row goes
    // with it — the same bound the door applies, from the same predicate.
    let mut e2 = AccessEngine::new();
    e2.set_profile(
        &space(),
        AccessProfile::new().allow(Role::Manager, "closeTask"),
    );
    e2.set_membership(&space(), &user("ada"), Role::Viewer);
    e2.set_membership(&space(), &PrincipalId("agt_ellanor".into()), Role::Viewer);
    let demoted = caller_pairs(&e2.callers_of(&space(), "closeTask", &[token], &[dlg], NOW));
    assert!(
        !demoted.iter().any(|(_, k, _)| k == "delegation"),
        "a delegation from somebody who cannot call the op is not a way to \
         call the op: {demoted:?}"
    );
}

#[test]
fn a_delegated_session_cannot_refresh_presence_into_itself() {
    // The step-up refresh carries "everything else about the session
    // through unchanged". A delegated token must not be one of the things
    // it carries: refreshing presence into one would make a delegation
    // satisfy a step-up gate, and step-up is the platform's ONLY mechanism
    // for "not delegable".
    let mut d = Directory::new(Provisioning::AutoOnboard);
    let subject = crate::provider::AuthenticatedSubject::without_presence(
        authr("oidc:google:ada"),
        AuthMethodKind::Oidc,
    );
    let (p, _) = d.resolve_or_onboard(&subject, "Ada", T0).unwrap();
    let claims = TokenClaims {
        principal: p,
        capability: None,
        client: None,
        user_presence_at: None,
        delegation: Some(ada_delegation(T1)),
        session: None,
    };
    assert!(matches!(
        crate::session::refresh_presence(&d, &claims, &subject, T1),
        Err(crate::session::SessionError::PresenceNotDelegable { .. })
    ));
}

// ───────────────────── durability and session lifetime ──────────────────
//
// The two halves of what 20b found missing under SSO: a credential that
// only ever lived in one process's heap, and a token that nothing could end
// or continue. Every test here was written against the old code first and
// every one of them failed there, most could not even compile, since
// `LocalPasswordProvider` had no store to share and `TokenSpec` had no
// session to name.

/// Counts what it was asked to do, so a test can assert on *work* rather
/// than on elapsed time. A timing assertion in a unit test is a flake; the
/// property that matters is that both paths run the same number of
/// verifications, and that is countable.
#[derive(Default)]
struct CountingHasher {
    hashes: std::sync::atomic::AtomicUsize,
    verifies: std::sync::atomic::AtomicUsize,
}

impl CountingHasher {
    fn verifies(&self) -> usize {
        self.verifies.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl crate::provider::PasswordHasher for std::sync::Arc<CountingHasher> {
    fn hash(&self, password: &str) -> String {
        self.hashes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("counted${password}")
    }

    fn verify(&self, password: &str, hash: &str) -> bool {
        self.verifies
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        hash.strip_prefix("counted$") == Some(password)
    }
}

fn answer_password(
    p: &dyn AuthProvider,
    identifier: &str,
    password: &str,
) -> Result<crate::provider::AuthenticatedSubject, AuthError> {
    let ChallengeOrRedirect::Challenge { id, .. } = p
        .begin(
            &BeginRequest {
                identifier: Some(identifier.into()),
                return_to: None,
                presence: false,
            },
            NOW,
        )
        .expect("begins")
    else {
        panic!("password is a challenge");
    };
    p.complete(
        &CompleteRequest::Password {
            challenge: id,
            identifier: identifier.into(),
            password: password.into(),
        },
        NOW,
    )
}

/// **The restart, in miniature.** The provider that enrolled the credential
/// is dropped entirely and a second one is built over the same store; the
/// credential is still there and still verifies.
///
/// This is the property the in-memory `BTreeMap` could not have: it was a
/// field, so the credential's life was the provider's life, which was the
/// process's life. The durable version of this assertion restarts against a
/// database in `ordo-host/tests/auth_durability.rs`.
#[test]
fn a_credential_outlives_the_provider_that_enrolled_it() {
    let store = std::sync::Arc::new(crate::credentials::MemoryCredentialStore::new());

    let first = LocalPasswordProvider::with_store(PlainTestHasher, store.clone());
    first.enroll("alice", "correct horse").expect("enrolled");
    assert_eq!(store.len(), 1);
    drop(first);

    let second = LocalPasswordProvider::with_store(PlainTestHasher, store.clone());
    let subject = answer_password(&second, "alice", "correct horse").expect("still enrolled");
    assert_eq!(subject.authenticator, authr("password:alice"));
}

/// The plaintext reaches the hasher and stops there. Nothing a store is
/// handed, and therefore nothing a store could log or dump, is the password.
#[test]
fn the_store_never_sees_a_password() {
    let store = std::sync::Arc::new(crate::credentials::MemoryCredentialStore::new());
    let p = LocalPasswordProvider::with_store(
        crate::provider::Argon2idHasher::weak_for_tests(),
        store.clone(),
    );
    p.enroll("alice", "correct horse").expect("enrolled");
    let stored = store
        .hash_for("alice")
        .expect("readable")
        .expect("enrolled");
    assert!(
        !stored.contains("correct horse"),
        "the stored value must not contain the password"
    );
    assert!(stored.starts_with("$argon2id$"), "a PHC string: {stored}");
}

/// **Not an existence oracle, and not a stopwatch one either.** An unknown
/// identifier and a wrong password both cost exactly one verification, so
/// the two refusals are indistinguishable by work as well as by wording.
///
/// This failed before the decoy: the unknown path returned without
/// verifying anything, so the count was 0 for a stranger and 1 for a real
/// account: a reliable account-existence answer over the network, whatever
/// the response body said.
#[test]
fn an_unknown_identifier_costs_the_same_work_as_a_wrong_password() {
    let hasher = std::sync::Arc::new(CountingHasher::default());
    let p = LocalPasswordProvider::new(hasher.clone());
    p.enroll("alice", "correct horse").expect("enrolled");
    let after_enrol = hasher.verifies();

    assert_eq!(
        answer_password(&p, "alice", "wrong"),
        Err(AuthError::BadCredentials)
    );
    let wrong_password = hasher.verifies() - after_enrol;

    assert_eq!(
        answer_password(&p, "nobody-here", "wrong"),
        Err(AuthError::BadCredentials)
    );
    let unknown_identifier = hasher.verifies() - after_enrol - wrong_password;

    assert_eq!(wrong_password, 1);
    assert_eq!(
        unknown_identifier, wrong_password,
        "an unknown identifier must cost exactly what a wrong password costs"
    );
}

/// A password that happens to match the decoy is still not a login. The
/// decoy is unguessable, so this is belt and braces, but the check that
/// makes it true is the `known` flag, and a refactor that dropped it would
/// pass every other test in this file.
#[test]
fn verifying_against_the_decoy_never_admits_anybody() {
    // A hasher that says yes to everything is the strongest form of the
    // question: if the decoy comparison alone could admit, this admits.
    struct AlwaysYes;
    impl crate::provider::PasswordHasher for AlwaysYes {
        fn hash(&self, _p: &str) -> String {
            "yes".into()
        }
        fn verify(&self, _p: &str, _h: &str) -> bool {
            true
        }
    }
    let p = LocalPasswordProvider::new(AlwaysYes);
    assert_eq!(
        answer_password(&p, "nobody-here", "anything"),
        Err(AuthError::BadCredentials),
        "no enrolment means no login, however agreeable the hasher"
    );
}

/// A store outage is its own answer. It does not depend on the identifier,
/// so it discloses nothing, and conflating it with a wrong password would
/// send every user to a password reset during a database incident.
#[test]
fn a_store_that_cannot_answer_is_not_a_wrong_password() {
    struct Broken;
    impl crate::credentials::CredentialStore for Broken {
        fn hash_for(
            &self,
            _i: &str,
        ) -> Result<Option<String>, crate::credentials::CredentialStoreError> {
            Err(crate::credentials::CredentialStoreError("no route".into()))
        }
        fn put(&self, _i: &str, _h: &str) -> Result<(), crate::credentials::CredentialStoreError> {
            Err(crate::credentials::CredentialStoreError("no route".into()))
        }
        fn remove(&self, _i: &str) -> Result<(), crate::credentials::CredentialStoreError> {
            Err(crate::credentials::CredentialStoreError("no route".into()))
        }
    }
    let p = LocalPasswordProvider::with_store(PlainTestHasher, std::sync::Arc::new(Broken));
    assert!(matches!(
        answer_password(&p, "alice", "correct horse"),
        Err(AuthError::StoreUnavailable(_))
    ));
}

fn session_claims(at: Timestamp) -> (TokenClaims, crate::session::SessionClaim) {
    let s = crate::session::SessionClaim::starting_at(at);
    (
        TokenClaims {
            principal: user("ada"),
            capability: None,
            client: Some(ClientId("cli_web".into())),
            user_presence_at: None,
            delegation: None,
            session: Some(s.clone()),
        },
        s,
    )
}

/// A session continues without a ceremony, and comes back no wider than it
/// went in.
#[test]
fn refresh_continues_a_session_and_widens_nothing() {
    let revocations = crate::session::MemorySessionRevocations::new();
    let (claims, s) = session_claims(T0);
    let later = Timestamp(T0.0 + 1_800);

    let spec = crate::session::refresh(
        &revocations,
        &claims,
        later,
        crate::session::DEFAULT_SESSION_SECS,
        crate::session::DEFAULT_SESSION_MAX_SECS,
    )
    .expect("a live session refreshes");

    assert_eq!(spec.principal, user("ada"));
    assert_eq!(spec.client, Some(ClientId("cli_web".into())));
    assert_eq!(
        spec.session.as_ref().map(|x| &x.id),
        Some(&s.id),
        "the same session, not a new one"
    );
    assert_eq!(
        spec.session.as_ref().map(|x| x.started_at),
        Some(T0),
        "and it still remembers when it started, so the ceiling cannot move"
    );
    assert_eq!(spec.expires, Timestamp(later.0 + 3_600));
    assert!(spec.capability.is_none());
    assert!(spec.delegation.is_none());
}

/// **The logged-out session is refused.** This is what "revocation rides
/// membership status" could not do: suspending the principal would have
/// ended every session they hold, on every device.
#[test]
fn a_logged_out_session_cannot_be_refreshed() {
    let revocations = crate::session::MemorySessionRevocations::new();
    let (claims, s) = session_claims(T0);

    let ended = crate::session::logout(
        &revocations,
        &claims,
        crate::session::DEFAULT_SESSION_MAX_SECS,
    )
    .expect("a session can always end itself");
    assert_eq!(ended, s.id);
    assert!(revocations.is_revoked(&s.id));

    assert_eq!(
        crate::session::refresh(
            &revocations,
            &claims,
            Timestamp(T0.0 + 10),
            crate::session::DEFAULT_SESSION_SECS,
            crate::session::DEFAULT_SESSION_MAX_SECS,
        ),
        Err(crate::session::LifetimeError::Revoked)
    );
}

/// Logging out twice is logging out. A client that retries a failed logout
/// must not be told the second one was an error.
#[test]
fn logout_is_idempotent() {
    let revocations = crate::session::MemorySessionRevocations::new();
    let (claims, _) = session_claims(T0);
    let max = crate::session::DEFAULT_SESSION_MAX_SECS;
    assert!(crate::session::logout(&revocations, &claims, max).is_ok());
    assert!(crate::session::logout(&revocations, &claims, max).is_ok());
    assert_eq!(revocations.len(), 1);
}

/// Refresh cannot outlive the ceiling, and the last token before it is
/// clipped to the ceiling rather than reaching past it.
#[test]
fn refresh_cannot_reach_past_the_ceiling() {
    let revocations = crate::session::MemorySessionRevocations::new();
    let (claims, _) = session_claims(T0);
    let max = 3_600 * 3;
    let ceiling = Timestamp(T0.0 + max);

    // Half an hour before the ceiling: allowed, and clipped to it.
    let nearly = Timestamp(ceiling.0 - 1_800);
    let spec = crate::session::refresh(&revocations, &claims, nearly, 3_600, max)
        .expect("still inside the ceiling");
    assert_eq!(
        spec.expires, ceiling,
        "a token minted near the ceiling ends at it, not an hour past it"
    );

    // At the ceiling, and past it: nothing to present.
    for at in [ceiling, Timestamp(ceiling.0 + 1)] {
        assert!(matches!(
            crate::session::refresh(&revocations, &claims, at, 3_600, max),
            Err(crate::session::LifetimeError::CeilingReached { .. })
        ));
    }
}

/// A capability token is not a session and is not logged out. Its authority
/// is the grant, and the grant is what gets withdrawn.
#[test]
fn a_capability_token_is_not_a_session() {
    let revocations = crate::session::MemorySessionRevocations::new();
    let claims = TokenClaims {
        principal: user("bot"),
        capability: Some(crate::principal::CapabilityGrant::new(
            GrantId("gr_1".into()),
            space(),
            std::collections::BTreeMap::new(),
            T1,
        )),
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };
    let max = crate::session::DEFAULT_SESSION_MAX_SECS;
    assert_eq!(
        crate::session::logout(&revocations, &claims, max),
        Err(crate::session::LifetimeError::NotASession)
    );
    assert_eq!(
        crate::session::refresh(&revocations, &claims, T0, 3_600, max),
        Err(crate::session::LifetimeError::NotASession)
    );
}

/// **Refresh does not launder a stale presence.** The stamp is carried
/// across unchanged, so it goes on ageing against the engine's step-up
/// window from the ceremony that made it, and an hourly refresh cannot keep
/// a step-up session alive all day.
#[test]
fn refresh_carries_presence_across_without_refreshing_it() {
    let revocations = crate::session::MemorySessionRevocations::new();
    let (mut claims, _) = session_claims(T0);
    claims.user_presence_at = Some(T0);

    let much_later = Timestamp(T0.0 + 7_200);
    let spec = crate::session::refresh(
        &revocations,
        &claims,
        much_later,
        3_600,
        crate::session::DEFAULT_SESSION_MAX_SECS,
    )
    .expect("inside the ceiling");
    assert_eq!(
        spec.user_presence_at,
        Some(T0),
        "the presence stamp is the ceremony's instant, not the refresh's"
    );
}

/// An entry may be dropped once every token naming it is dead anyway, and
/// not a second before.
#[test]
fn a_revocation_outlives_every_token_that_could_use_it() {
    let revocations = crate::session::MemorySessionRevocations::new();
    let (claims, s) = session_claims(T0);
    let max = 3_600;
    crate::session::logout(&revocations, &claims, max).expect("ended");

    let ceiling = Timestamp(T0.0 + max);
    revocations.prune(Timestamp(ceiling.0 - 1));
    assert!(
        revocations.is_revoked(&s.id),
        "an entry must not be pruned while a token naming it could still verify"
    );
    revocations.prune(ceiling);
    assert!(!revocations.is_revoked(&s.id));
}

/// The session claim survives the token encoding, which is what makes every
/// test above reachable from a bearer string rather than only from a struct.
#[test]
fn the_session_claim_survives_the_token_round_trip() {
    let key = crate::token::TokenKey::from_secret(b"deployment-secret");
    let s = crate::session::SessionClaim::starting_at(T0);
    let spec = crate::token::TokenSpec {
        principal: user("ada"),
        expires: T1,
        client: None,
        user_presence_at: None,
        capability: None,
        delegation: None,
        session: Some(s.clone()),
    };
    let claims = crate::token::verify(&key, &crate::token::mint(&key, &spec), T0).expect("valid");
    assert_eq!(claims.session, Some(s));
}

// ── A deleted account (owner ruling 2026-10-06) ──────────────────────────

/// A deleted account is marked and kept. Its authenticator still names it,
/// so signing in with it is refused as deleted: it neither signs in nor
/// onboards a stranger in its place.
#[test]
fn a_deleted_account_is_refused_at_sign_in_and_is_not_onboarded_again() {
    let mut dir = Directory::new(Provisioning::AutoOnboard);
    let subject = crate::provider::AuthenticatedSubject {
        authenticator: AuthenticatorId("password:dana".into()),
        method: crate::AuthMethodKind::Password,
        attributes: Default::default(),
        presence: None,
        upstream_presence: None,
    };
    let first = crate::session::sign_in(&mut dir, &subject, "Dana", Timestamp(10))
        .expect("signs in")
        .principal;
    dir.mark_deleted(&first, Timestamp(20)).expect("marked");
    assert!(matches!(
        crate::session::sign_in(&mut dir, &subject, "Dana", Timestamp(30)),
        Err(DirectoryError::Deleted(p)) if p == first
    ));
    assert_eq!(
        dir.principal_for(&subject.authenticator).unwrap(),
        Some(first.clone()),
        "the identifier stays linked to the deleted account"
    );
    assert!(
        dir.principal(&first).unwrap().is_some(),
        "the account is kept"
    );
}

/// A deleted account is given no new authenticator: `link_first`, the
/// path a ticket that names the person takes, refuses it as deleted rather
/// than linking a fresh passkey that would then sign in (audit 2026-10-06).
#[test]
fn a_deleted_account_is_given_no_new_authenticator_link() {
    let mut dir = Directory::new(Provisioning::InviteOnly);
    let person = dir
        .create_principal(PrincipalKind::Human, "Dana", Timestamp(10))
        .expect("minted");
    dir.mark_deleted(&person, Timestamp(20)).expect("marked");
    let new = AuthenticatorId("passkey:fresh".into());
    assert!(matches!(
        dir.link_first(&person, &new, Timestamp(30)),
        Err(DirectoryError::Deleted(p)) if p == person
    ));
    assert_eq!(dir.principal_for(&new).unwrap(), None, "nothing was linked");
}
