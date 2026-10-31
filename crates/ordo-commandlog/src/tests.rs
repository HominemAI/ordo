//! Unit tests: chain append/verify, tamper detection, revert inversion,
//! actor attribution, sealed-field redaction, and group shape gating.

use crate::chain::{encode_image_bytes, verify_chain_from, ChainHash, TamperKind};
use crate::command::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, GroupShapeError, SpaceId,
    Timestamp,
};
use crate::store::{
    checkpoint, revert_group, AppendError, Checkpoint, CheckpointError, InMemoryStore, RevertError,
    Store, StubSigner,
};
use crate::value::{Image, Value};

const T0: Timestamp = Timestamp(1_000);
const T9: Timestamp = Timestamp(9_000);

fn space() -> SpaceId {
    SpaceId("spc_01".into())
}

/// Another tenant's space in the same deployment.
fn other_space() -> SpaceId {
    SpaceId("spc_02".into())
}

fn reverter() -> Actor {
    Actor::User("usr_reverter".into())
}

/// The same group, appended to `space` instead of the default one.
fn create_group_in(space: &SpaceId, correlation: &str) -> CommandGroup {
    let mut group = create_group(correlation);
    for c in &mut group.commands {
        c.space = space.clone();
    }
    group
}

fn img(fields: &[(&str, Value)]) -> Image {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn cmd(id: &str, kind: CommandKind, before: Option<Image>, after: Option<Image>) -> Command {
    Command {
        id: CommandId(id.into()),
        space: space(),
        entity: "transactions".into(),
        row_id: "txn_01".into(),
        kind,
        before,
        after,
        at: T0,
    }
}

fn create_group(correlation: &str) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(correlation.into()),
        actor: Actor::User("usr_x".into()),
        commands: vec![cmd(
            &format!("cmd-{correlation}"),
            CommandKind::Create,
            None,
            Some(img(&[
                ("amount", Value::Int(-1050)),
                ("cleared", Value::Bool(false)),
            ])),
        )],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

// ───────────────────────── chain append/verify ──────────────────────────

#[test]
fn append_links_and_verifies() {
    let mut store = InMemoryStore::new();
    let e1 = store.append(create_group("grp-1")).unwrap();
    let e2 = store.append(create_group("grp-2")).unwrap();

    assert_eq!(e1.prev, ChainHash::GENESIS);
    assert_eq!(e2.prev, e1.hash);
    assert_eq!(store.head(&space()), Some(e2.hash));
    assert_eq!(store.len(&space()), 2);
    store.verify(&space()).expect("untouched chain verifies");
}

#[test]
fn pruned_suffix_verifies_against_its_checkpoint() {
    // A lawful prune must not read as tampering. Verifying a suffix from
    // genesis reports BrokenLink at the cut; verifying it from the
    // checkpoint's head is the same chain, anchored where it actually starts.
    let mut store = InMemoryStore::new();
    let e1 = store.append(create_group("grp-1")).unwrap();
    let e2 = store.append(create_group("grp-2")).unwrap();
    let e3 = store.append(create_group("grp-3")).unwrap();

    // Prune everything through e1; a checkpoint at upto=1 covers it.
    let suffix = vec![e2.clone(), e3.clone()];

    let from_genesis = verify_chain_from(&space(), &suffix, ChainHash::GENESIS, 0);
    let err = from_genesis.expect_err("genesis anchor cannot verify a pruned suffix");
    assert_eq!(err.kind, TamperKind::BrokenLink);
    assert_eq!(err.index, 0);

    verify_chain_from(&space(), &suffix, e1.hash, 1)
        .expect("pruned suffix verifies against the checkpoint head");

    // The reported index still names a position in the full chain.
    let mut tampered = suffix.clone();
    tampered[1].hash = ChainHash::GENESIS;
    let err = verify_chain_from(&space(), &tampered, e1.hash, 1)
        .expect_err("tampering inside the suffix is still caught");
    assert_eq!(err.index, 2, "index is absolute, not relative to the cut");
}

#[test]
fn chains_are_per_space() {
    let mut store = InMemoryStore::new();
    store.append(create_group("grp-a")).unwrap();
    let mut other = create_group("grp-b");
    for c in &mut other.commands {
        c.space = SpaceId("spc_02".into());
    }
    let e = store.append(other).unwrap();
    // The second space starts its own chain at genesis.
    assert_eq!(e.prev, ChainHash::GENESIS);
    store.verify(&space()).unwrap();
    store.verify(&SpaceId("spc_02".into())).unwrap();
    assert!(store.verify(&SpaceId("spc_empty".into())).is_ok());
}

#[test]
fn duplicate_correlation_rejected() {
    let mut store = InMemoryStore::new();
    store.append(create_group("grp-1")).unwrap();
    assert_eq!(
        store.append(create_group("grp-1")),
        Err(AppendError::DuplicateCorrelation(CorrelationId(
            "grp-1".into()
        )))
    );
}

#[test]
fn correlation_uniqueness_is_per_space_not_global() {
    // SH2-14: a global correlation namespace lets a write in one tenant's
    // space permanently deny that id in another's.
    let mut store = InMemoryStore::new();
    store.append(create_group("grp-1")).unwrap();
    store
        .append(create_group_in(&other_space(), "grp-1"))
        .expect("the same correlation id is free in another space");
    assert_eq!(store.len(&space()), 1);
    assert_eq!(store.len(&other_space()), 1);
}

// ───────────────────── space-scoped lookup (Y-032) ──────────────────────

#[test]
fn find_is_invisible_across_spaces() {
    let mut store = InMemoryStore::new();
    store.append(create_group("grp-a")).unwrap();

    // The owning space sees it…
    let found = store
        .find(&space(), &CorrelationId("grp-a".into()))
        .expect("own space finds it");
    assert_eq!(found.group.correlation, CorrelationId("grp-a".into()));
    // …and a lookup from any other space does not, images included.
    assert!(store
        .find(&other_space(), &CorrelationId("grp-a".into()))
        .is_none());
    assert!(store
        .find(
            &SpaceId("spc_unknown".into()),
            &CorrelationId("grp-a".into())
        )
        .is_none());
    // The existence probe is a boolean and is scoped the same way.
    assert!(store.is_logged_elsewhere(&other_space(), &CorrelationId("grp-a".into())));
    assert!(!store.is_logged_elsewhere(&space(), &CorrelationId("grp-a".into())));
}

#[test]
fn revert_from_another_space_is_a_typed_error() {
    let mut store = InMemoryStore::new();
    store.append(create_group("grp-a")).unwrap();
    // A guessed or leaked correlation id used from a space that does not own
    // the group yields a named refusal — not the group's before/after images.
    assert_eq!(
        revert_group(
            &store,
            &other_space(),
            &CorrelationId("grp-a".into()),
            &reverter(),
            T9
        ),
        Err(RevertError::CrossSpace {
            correlation: CorrelationId("grp-a".into()),
            requested: other_space(),
        })
    );
    // A correlation id nobody logged anywhere stays an ordinary unknown.
    assert_eq!(
        revert_group(
            &store,
            &other_space(),
            &CorrelationId("grp-ghost".into()),
            &reverter(),
            T9
        ),
        Err(RevertError::UnknownGroup(CorrelationId("grp-ghost".into())))
    );
    // Same id in both spaces: each space reverts its own group.
    store
        .append(create_group_in(&other_space(), "grp-a"))
        .unwrap();
    let plan = revert_group(
        &store,
        &other_space(),
        &CorrelationId("grp-a".into()),
        &reverter(),
        T9,
    )
    .expect("its own space reverts");
    assert!(plan.commands.iter().all(|c| c.space == other_space()));
}

// ─────────────────────────── tamper detection ───────────────────────────

#[test]
fn mutated_image_fails_verify_at_right_index() {
    let mut store = InMemoryStore::new();
    store.append(create_group("grp-0")).unwrap();
    store.append(create_group("grp-1")).unwrap();
    store.append(create_group("grp-2")).unwrap();

    // Tamper with the stored after-image of the middle group.
    let entry = &mut store.chain_mut(&space())[1];
    entry.group.commands[0]
        .after
        .as_mut()
        .unwrap()
        .insert("amount".into(), Value::Int(999_999));

    let err = store.verify(&space()).unwrap_err();
    assert_eq!(err.space, space());
    assert_eq!(err.index, 1);
    assert_eq!(err.kind, TamperKind::HashMismatch);
}

#[test]
fn broken_link_detected() {
    let mut store = InMemoryStore::new();
    store.append(create_group("grp-0")).unwrap();
    store.append(create_group("grp-1")).unwrap();

    // Rewrite the second entry's prev pointer.
    store.chain_mut(&space())[1].prev = ChainHash([7u8; 32]);

    let err = store.verify(&space()).unwrap_err();
    assert_eq!(err.index, 1);
    assert_eq!(err.kind, TamperKind::BrokenLink);
}

#[test]
fn rewritten_actor_fails_verify() {
    // Attribution is hashed: repointing blame is tampering.
    let mut store = InMemoryStore::new();
    store.append(create_group("grp-0")).unwrap();
    store.chain_mut(&space())[0].group.actor = Actor::User("usr_scapegoat".into());
    let err = store.verify(&space()).unwrap_err();
    assert_eq!(err.index, 0);
    assert_eq!(err.kind, TamperKind::HashMismatch);
}

// ─────────────────────────── revert planning ────────────────────────────

#[test]
fn revert_inverts_all_three_kinds_in_reverse_order() {
    let before_conv = img(&[("category", Value::Id("cat_old".into()))]);
    let after_conv = img(&[("category", Value::Id("cat_new".into()))]);
    let created = img(&[("amount", Value::Int(-500))]);
    let deleted = img(&[("amount", Value::Int(42)), ("notes", Value::Null)]);

    let group = CommandGroup {
        correlation: CorrelationId("grp-mix".into()),
        actor: Actor::Routine("receipt-matcher".into()),
        commands: vec![
            cmd("c-create", CommandKind::Create, None, Some(created.clone())),
            cmd(
                "c-conv",
                CommandKind::Converge,
                Some(before_conv.clone()),
                Some(after_conv.clone()),
            ),
            cmd("c-del", CommandKind::Delete, Some(deleted.clone()), None),
        ],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    };
    let mut store = InMemoryStore::new();
    store.append(group).unwrap();

    let group = revert_group(
        &store,
        &space(),
        &CorrelationId("grp-mix".into()),
        &reverter(),
        T9,
    )
    .unwrap();
    // The inverse is itself a group: attributed to whoever asked for the
    // revert, correlated to the group it undoes.
    assert_eq!(group.actor, reverter());
    assert_eq!(group.correlation, CorrelationId("rev-grp-mix".into()));
    let plan = &group.commands;
    assert_eq!(plan.len(), 3);

    // Reverse order: the delete is undone first.
    assert_eq!(plan[0].kind, CommandKind::Create);
    assert_eq!(plan[0].before, None);
    assert_eq!(plan[0].after, Some(deleted));
    assert_eq!(plan[0].id, CommandId("rev-c-del".into()));

    // Converge with the before-image: images swapped.
    assert_eq!(plan[1].kind, CommandKind::Converge);
    assert_eq!(plan[1].before, Some(after_conv));
    assert_eq!(plan[1].after, Some(before_conv));

    // The create is undone last, by deleting what it made.
    assert_eq!(plan[2].kind, CommandKind::Delete);
    assert_eq!(plan[2].before, Some(created));
    assert_eq!(plan[2].after, None);

    // The plan is well-shaped history itself, stamped now, not applied.
    group
        .validate()
        .expect("the inverse is a well-shaped, single-space group");
    for c in plan {
        assert_eq!(c.at, T9);
    }
    assert_eq!(
        store.len(&space()),
        1,
        "revert planning must not apply anything"
    );
}

/// Inversion carries a `SealedRef` through; it does not refuse one.
///
/// This crate is the arithmetic and no I/O. Whether a handle resolves is a
/// question about a deployment — does the space still hold the payload, is
/// the epoch key still alive — and answering it from here would mean
/// guessing. It used to guess "no", because before the sealed payload
/// store existed (8j) that guess was right for every handle; now it would
/// be wrong for almost all of them, and refusing a revert that the
/// platform can perfectly well perform is a real cost to whoever needs
/// their write undone.
///
/// The refusal itself has not gone anywhere — see
/// `ordo_exec::undo`, which resolves each handle against the store and
/// raises `RevertError::UnresolvableImage` when the payload was pruned or
/// its epoch key destroyed. What changed is who is entitled to decide.
#[test]
fn inversion_carries_a_sealed_handle_through_for_the_applier_to_resolve() {
    let before_ref = [7u8; 16];
    let after_ref = [8u8; 16];
    let group = CommandGroup {
        correlation: CorrelationId("grp-sealed".into()),
        actor: Actor::User("usr_1".into()),
        commands: vec![cmd(
            "c-conv",
            CommandKind::Converge,
            Some(img(&[
                ("memo", Value::SealedRef(before_ref)),
                ("note", Value::Text("plain".into())),
            ])),
            Some(img(&[
                ("memo", Value::SealedRef(after_ref)),
                ("note", Value::Text("plain".into())),
            ])),
        )],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    };
    let mut store = InMemoryStore::new();
    store.append(group).unwrap();

    let inverse = revert_group(
        &store,
        &space(),
        &CorrelationId("grp-sealed".into()),
        &reverter(),
        T9,
    )
    .expect("inversion no longer refuses a handle it cannot itself resolve");

    // Converge inverts by swapping the images, so the value the revert
    // will WRITE is the original before-image's handle, and the value it
    // expects to find is the after-image's. Both survive the swap
    // byte-for-byte: an inversion that re-minted a handle would be naming
    // a payload nobody stored.
    let c = &inverse.commands[0];
    assert_eq!(
        c.after.as_ref().unwrap()["memo"],
        Value::SealedRef(before_ref)
    );
    assert_eq!(
        c.before.as_ref().unwrap()["memo"],
        Value::SealedRef(after_ref)
    );
}

#[test]
fn revert_of_unknown_group_is_an_error() {
    let store = InMemoryStore::new();
    assert_eq!(
        revert_group(
            &store,
            &space(),
            &CorrelationId("grp-ghost".into()),
            &reverter(),
            T9
        ),
        Err(RevertError::UnknownGroup(CorrelationId("grp-ghost".into())))
    );
}

// ──────────────────────── actor attribution ─────────────────────────────

#[test]
fn actor_attribution_formats_canonically() {
    assert_eq!(Actor::User("usr_x".into()).to_string(), "user:usr_x");
    assert_eq!(
        Actor::Routine("spend-watch".into()).to_string(),
        "routine:spend-watch"
    );
    assert_eq!(
        Actor::AgentOnBehalfOf {
            agent: "ellanor".into(),
            user: "usr_x".into()
        }
        .to_string(),
        "agent:ellanor on behalf of user:usr_x"
    );
    assert_eq!(
        Actor::Source("camera-hub".into()).to_string(),
        "source:camera-hub"
    );
    assert_eq!(
        Actor::System("retention".into()).to_string(),
        "system:retention"
    );
}

// ───────────────────────── sealed-field redaction ───────────────────────

#[test]
fn sealed_fields_never_carry_plaintext() {
    // The sealed field's image holds only an opaque handle; the canonical
    // encoding — the exact bytes the chain hashes and stores — must not
    // contain the plaintext anywhere.
    let secret = "payee: Dr. Real Name, London";
    let sealed_ref = [0xABu8; 16]; // stands in for a random payload handle
    let image = img(&[
        ("amount", Value::Int(-1050)),
        ("payee", Value::SealedRef(sealed_ref)),
    ]);

    let bytes = encode_image_bytes(&image);
    let needle = secret.as_bytes();
    assert!(
        !bytes.windows(needle.len()).any(|w| w == needle),
        "plaintext leaked into the canonical encoding"
    );
    // The reference itself is present — the log can name the ciphertext.
    assert!(bytes.windows(16).any(|w| w == sealed_ref));
}

// ───────────────────────── group shape gating ───────────────────────────

#[test]
fn malformed_image_shapes_rejected_at_append() {
    let mut store = InMemoryStore::new();

    // Create with a before-image: not a create.
    let bad = CommandGroup {
        correlation: CorrelationId("grp-bad".into()),
        actor: Actor::User("usr_x".into()),
        commands: vec![cmd(
            "c1",
            CommandKind::Create,
            Some(img(&[])),
            Some(img(&[])),
        )],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    };
    match store.append(bad) {
        Err(AppendError::Shape(GroupShapeError::BadImageShape { command, kind, .. })) => {
            assert_eq!(command, CommandId("c1".into()));
            assert_eq!(kind, CommandKind::Create);
        }
        other => panic!("expected BadImageShape, got {other:?}"),
    }

    // Empty group.
    let empty = CommandGroup {
        correlation: CorrelationId("grp-empty".into()),
        actor: Actor::User("usr_x".into()),
        commands: vec![],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    };
    assert!(matches!(
        store.append(empty),
        Err(AppendError::Shape(GroupShapeError::EmptyGroup { .. }))
    ));

    // Mixed spaces: a group is one transaction in one space.
    let mut mixed = create_group("grp-mixed");
    mixed.commands.push({
        let mut c = cmd("c2", CommandKind::Create, None, Some(img(&[])));
        c.space = SpaceId("spc_02".into());
        c
    });
    assert!(matches!(
        store.append(mixed),
        Err(AppendError::Shape(GroupShapeError::MixedSpaces { .. }))
    ));
}

// ────────────────────────────── checkpoints ─────────────────────────────

#[test]
fn checkpoint_signs_the_head_and_verifies() {
    let mut store = InMemoryStore::new();
    store.append(create_group("grp-0")).unwrap();
    store.append(create_group("grp-1")).unwrap();
    let signer = StubSigner {
        key_id: "chk-key-1".into(),
    };

    let cp = checkpoint(&store, &space(), &signer, T9).unwrap();
    assert_eq!(cp.space, space());
    assert_eq!(cp.upto, 2);
    assert_eq!(Some(cp.head), store.head(&space()));
    assert_eq!(cp.key_id, "chk-key-1");
    assert!(cp.verify(&signer));

    // A doctored checkpoint fails signature verification.
    let mut forged = cp.clone();
    forged.upto = 1;
    assert!(!forged.verify(&signer));
    // And a different key does not vouch for it.
    let other = StubSigner {
        key_id: "chk-key-2".into(),
    };
    assert!(!cp.verify(&other));
}

#[test]
fn checkpoint_of_empty_chain_is_an_error() {
    let store = InMemoryStore::new();
    let signer = StubSigner { key_id: "k".into() };
    assert_eq!(
        checkpoint(&store, &SpaceId("spc_new".into()), &signer, T0),
        Err(CheckpointError::EmptyChain(SpaceId("spc_new".into())))
    );
}

// ─────────────────────── canonical decoding (8e) ────────────────────────

/// The property a durable store rests on: what came out is what went in.
/// If this is ever false, `verify` is checking a different artifact from
/// the one the chain hashed, and the audit spine means nothing.
#[test]
fn encoding_round_trips_every_shape() {
    let mut group = create_group("grp-round");
    // One command of each kind, images carrying every value tag.
    group.commands = vec![
        Command {
            id: CommandId("c-create".into()),
            space: space(),
            entity: "transactions".into(),
            row_id: "txn_01".into(),
            kind: CommandKind::Create,
            before: None,
            after: Some(img(&[
                ("amount", Value::Int(-42)),
                ("id", Value::Id("txn_01".into())),
                ("memo", Value::SealedRef([7u8; 16])),
                ("note", Value::Text("héllo — ünicode".into())),
                ("settled", Value::Bool(true)),
                ("void", Value::Null),
            ])),
            at: T0,
        },
        Command {
            id: CommandId("c-converge".into()),
            space: space(),
            entity: "transactions".into(),
            row_id: "txn_01".into(),
            kind: CommandKind::Converge,
            before: Some(img(&[("amount", Value::Int(i64::MIN))])),
            after: Some(img(&[("amount", Value::Int(i64::MAX))])),
            at: T9,
        },
        Command {
            id: CommandId("c-delete".into()),
            space: space(),
            entity: "transactions".into(),
            row_id: "txn_01".into(),
            kind: CommandKind::Delete,
            before: Some(img(&[])),
            after: None,
            at: T0,
        },
    ];
    let bytes = crate::chain::encode_group(&group);
    assert_eq!(crate::chain::decode_group(&bytes), Ok(group.clone()));
    // And the decoded group hashes to the same place, which is the only
    // form of "identical" the chain actually cares about.
    assert_eq!(
        crate::chain::hash_group(ChainHash::GENESIS, &group),
        crate::chain::hash_group(
            ChainHash::GENESIS,
            &crate::chain::decode_group(&bytes).unwrap()
        )
    );
}

// ───────────── the owner-administration flag (DL §3 rule 6) ─────────────

/// **The backward-compatibility proof, and it is a number rather than an
/// argument.**
///
/// `6ef5726c…` is the chain hash of `create_group("grp-c")` at genesis,
/// computed and written down **before** `owner_administration` existed on
/// [`CommandGroup`] — with the encoder that had never heard of a trailer.
/// If adding the field had moved a single byte of an ordinary group's
/// rendering, this constant would not match, and every chain, checkpoint
/// and signature ever written over an unflagged group would be void.
///
/// It is pinned here rather than derived, because a golden derived from the
/// current code proves nothing about the previous code.
const PRE_FLAG_GOLDEN: &str = "6ef5726ca36a88d2343735fd5acc1e188298ee247fd61a50f11a8f7da76a682a";

/// 163 bytes, also measured before the field existed. Pinned alongside the
/// hash because a length is the more legible failure: a hash that moved says
/// "something changed", a length that moved says "you added a byte".
const PRE_FLAG_LEN: usize = 163;

#[test]
fn an_unflagged_group_hashes_exactly_where_it_did_before_the_flag_existed() {
    let group = create_group("grp-c");
    assert!(!group.owner_administration);
    let bytes = crate::chain::encode_group(&group);
    assert_eq!(
        bytes.len(),
        PRE_FLAG_LEN,
        "an unflagged group must render the bytes it rendered before the \
         field existed — not one more"
    );
    assert_eq!(
        crate::chain::hash_group(ChainHash::GENESIS, &group).to_string(),
        PRE_FLAG_GOLDEN,
        "adding the flag moved an existing group's hash; every chain already \
         written is now unverifiable"
    );
}

/// The other direction: a rendering written by the old encoder — which is
/// simply an unflagged rendering, because they are the same bytes — decodes
/// under the new one, and decodes as *not* owner administration.
///
/// This is what makes the field safe to add to a frozen layout at all. There
/// is no format version, no migration and no fallback path: the old bytes
/// are a lawful current encoding.
#[test]
fn a_legacy_rendering_decodes_as_unflagged_and_re_encodes_identically() {
    let group = create_group("grp-legacy");
    let bytes = crate::chain::encode_group(&group);
    let decoded = crate::chain::decode_group(&bytes).expect("decodes");
    assert!(
        !decoded.owner_administration,
        "silence means no flag, which is what every stored group says"
    );
    assert_eq!(crate::chain::encode_group(&decoded), bytes);
}

/// A flagged group round-trips, and its rendering is the unflagged one plus
/// exactly one byte — the property the whole encoding argument rests on.
#[test]
fn a_flagged_group_round_trips_and_costs_one_byte() {
    let plain = create_group("grp-r6");
    let mut flagged = plain.clone();
    flagged.owner_administration = true;

    let plain_bytes = crate::chain::encode_group(&plain);
    let flagged_bytes = crate::chain::encode_group(&flagged);
    assert_eq!(flagged_bytes.len(), plain_bytes.len() + 1);
    assert_eq!(&flagged_bytes[..plain_bytes.len()], &plain_bytes[..]);
    assert_eq!(*flagged_bytes.last().expect("a trailer"), 0x01);

    assert_eq!(crate::chain::decode_group(&flagged_bytes), Ok(flagged));

    // And the two hash to different places, which is the point: the flag is
    // covered by the chain, so it cannot be added to or removed from a
    // stored group after the fact without the hash saying so.
    assert_ne!(
        crate::chain::hash_group(ChainHash::GENESIS, &plain),
        crate::chain::hash_group(
            ChainHash::GENESIS,
            &crate::chain::decode_group(&flagged_bytes).unwrap()
        )
    );
}

/// The flag rides the chain, so editing it after the fact is tampering and
/// reads as tampering — not as a different opinion about the same group.
///
/// This is the property that makes the flag worth having. An
/// owner-administration record an operator could quietly clear (or forge)
/// would answer the auditor's question with whatever the last writer
/// preferred.
#[test]
fn stripping_the_flag_from_a_stored_group_is_caught_as_tampering() {
    let mut store = InMemoryStore::new();
    let mut flagged = create_group("grp-r6-stored");
    flagged.owner_administration = true;
    let stored = store.append(flagged).expect("appends");

    // What a tamperer would do: keep the bytes, drop the trailer.
    let mut stripped = stored.clone();
    stripped.group.owner_administration = false;
    assert_ne!(
        crate::chain::hash_group(stripped.prev, &stripped.group),
        stored.hash,
        "the flag is inside the hash, so removing it breaks the chain"
    );
}

/// Canonicity. "No flags" has exactly one spelling — silence — so an
/// explicit `0x00` trailer is not a second way to say it, and neither is any
/// byte that is not one of the two real tags, in one of the two positions
/// `decode_group` accepts.
///
/// `0x02` is deliberately absent from the padding list this test pads an
/// otherwise-unflagged group's bytes with: since D7 it is not padding, it
/// is [`TRAILER_RESTORED_UNVERIFIED`][crate::chain] and decodes to a real
/// flag. `an_unknown_trailer_tag_still_refuses` and
/// `a_second_trailer_position_still_refuses_an_explicit_absence` cover the
/// two things that changed; this test covers what did not.
///
/// Without this, one group would have two lawful renderings and therefore
/// two hashes, which is the property a hash chain cannot survive losing.
#[test]
fn only_one_trailer_byte_exists_and_no_other_padding_is_lawful() {
    use crate::chain::{decode_group, DecodeError};
    let good = crate::chain::encode_group(&create_group("grp-canon"));

    for pad in [0x00u8, 0xff] {
        let mut padded = good.clone();
        padded.push(pad);
        assert_eq!(
            decode_group(&padded),
            Err(DecodeError::TrailingBytes),
            "0x{pad:02x} must not be a second spelling of anything"
        );
    }
    // A trailer is one byte and the last one: repeating the same tag is
    // not a second flag, it is one flag stated twice.
    let mut twice = good.clone();
    twice.push(0x01);
    twice.push(0x01);
    assert_eq!(decode_group(&twice), Err(DecodeError::TrailingBytes));
}

/// The companion proof for the second tag: `0x02` alone, on a group flagged
/// neither way, decodes as `restored_unverified` and nothing else: it is
/// not trailing, and it does not also claim `owner_administration`.
///
/// This is the reading `only_one_trailer_byte_exists_and_no_other_padding_
/// is_lawful` explicitly stopped asserting about `0x02` once it became a
/// real tag; this test asserts the other half, that it decodes to exactly
/// the flag it names.
#[test]
fn the_second_tag_alone_decodes_as_restored_unverified_alone() {
    use crate::chain::decode_group;
    let mut bytes = crate::chain::encode_group(&create_group("grp-second-alone"));
    bytes.push(0x02);
    let decoded = decode_group(&bytes).expect("0x02 alone is a lawful trailer");
    assert!(!decoded.owner_administration);
    assert!(decoded.restored_unverified);
}

/// Unknown trailer tags are refused rather than skipped, for any byte that
/// is not one of the tags this decoder knows, including a byte that looks
/// like a plausible next tag (keeping the ascending-order shape a future
/// flag would use) appended after the real tags are already present.
///
/// This is the property `plan-opslog.md` D7 depended on for its own claim,
/// and R2a then exercised: an older binary meeting a trailer some later
/// increment adds must refuse the group as corrupt rather than silently
/// drop the flag it does not recognize.
///
/// **Amended by R2a (`plan-production-ops.md`).** This test used `0x03` as
/// its "future" tag. `0x03` is now the restore marker, a known tag followed
/// by a payload, so a bare `0x03` still refuses, as a payload cut short
/// (`Truncated`) rather than as an unknown tag. The unknown-tag claim moves
/// to `0x04`, the next future tag, with the same assertion it made before.
#[test]
fn an_unknown_trailer_tag_still_refuses() {
    use crate::chain::{decode_group, DecodeError};

    // An unrecognized tag on a group flagged neither way.
    let mut unflagged = crate::chain::encode_group(&create_group("grp-unknown-tag"));
    unflagged.push(0x05);
    assert_eq!(decode_group(&unflagged), Err(DecodeError::TrailingBytes));

    // The next unrecognized tag after both flag tags are already present,
    // a future flag's shape, refused the same way.
    let mut both = create_group("grp-unknown-tag-2");
    both.owner_administration = true;
    both.restored_unverified = true;
    let both_bytes = crate::chain::encode_group(&both);
    let mut next = both_bytes.clone();
    next.push(0x04);
    assert_eq!(
        decode_group(&next),
        Err(DecodeError::TrailingBytes),
        "an unrecognized tag after the real ones must still refuse"
    );

    // What this test used to push, `0x03` with nothing after it, still
    // refuses: the restore tag without its payload is a group cut short.
    let mut bare_restore = both_bytes;
    bare_restore.push(0x03);
    assert_eq!(decode_group(&bare_restore), Err(DecodeError::Truncated));
}

/// The second position gets the same canonicity rule the first did: with
/// `owner_administration` already set, an explicit `0x00` naming "no second
/// flag" is refused exactly like an explicit `0x00` was refused for the
/// first tag on its own: silence is still the only spelling of absence,
/// now in either position.
#[test]
fn a_second_trailer_position_still_refuses_an_explicit_absence() {
    use crate::chain::{decode_group, DecodeError};
    let mut flagged = create_group("grp-explicit-absence");
    flagged.owner_administration = true;
    let mut bytes = crate::chain::encode_group(&flagged);
    assert_eq!(
        *bytes.last().unwrap(),
        0x01,
        "only the first tag is present yet"
    );
    bytes.push(0x00);
    assert_eq!(
        decode_group(&bytes),
        Err(DecodeError::TrailingBytes),
        "0x00 must not be a second flag's way of saying it is absent"
    );
}

/// A flagged group round-trips: encode a group whose restore had nothing to
/// verify, decode it back, and the flag survives: the property the whole
/// D7 trailer exists to give a durable home to.
#[test]
fn a_restored_unverified_group_round_trips() {
    use crate::chain::decode_group;
    let mut flagged = create_group("grp-d7-roundtrip");
    flagged.restored_unverified = true;
    let bytes = crate::chain::encode_group(&flagged);
    assert_eq!(decode_group(&bytes), Ok(flagged));
}

/// A revert never inherits the flag. The flag says how *this* call was
/// authorized, not what it undid — and a revert is a separate call, by a
/// separate actor, that rule 6 did not authorize.
#[test]
fn a_revert_of_an_owner_administration_group_is_not_itself_flagged() {
    let mut store = InMemoryStore::new();
    let mut flagged = create_group("grp-r6-revert");
    flagged.owner_administration = true;
    store.append(flagged).expect("appends");

    let inverse = revert_group(
        &store,
        &space(),
        &CorrelationId("grp-r6-revert".into()),
        &reverter(),
        T9,
    )
    .expect("inverts");
    assert!(
        !inverse.owner_administration,
        "a revert is authorized on its own terms; claiming rule 6 for it \
         would put a decision in the chain that no door ever made"
    );
}

#[test]
fn every_actor_form_round_trips() {
    for actor in [
        Actor::User("usr_1".into()),
        Actor::Routine("spend-watch".into()),
        Actor::AgentOnBehalfOf {
            agent: "ellanor".into(),
            user: "usr_1".into(),
        },
        Actor::Source("camera-hub".into()),
        Actor::System("retention".into()),
    ] {
        assert_eq!(Actor::parse(&actor.to_string()), Ok(actor));
    }
}

/// Attribution that is not one of the five forms is refused, not guessed.
/// An entry nobody can attribute is worse than a loud decode failure.
#[test]
fn unattributable_actor_is_refused() {
    for bad in [
        "",
        "usr_1",
        "admin:root",
        "user:",
        "agent:ellanor",
        "agent:",
    ] {
        assert!(
            matches!(
                Actor::parse(bad),
                Err(crate::chain::DecodeError::BadActor(_))
            ),
            "`{bad}` must not parse as an actor"
        );
    }
}

/// A hostile blob must fail typed rather than allocate what its own length
/// prefix claims.
#[test]
fn corrupt_encodings_fail_typed() {
    use crate::chain::{decode_group, DecodeError};
    let good = crate::chain::encode_group(&create_group("grp-c"));

    assert_eq!(decode_group(&[]), Err(DecodeError::Truncated));
    // A u64 length of 2^63 over an eight-byte buffer.
    assert_eq!(
        decode_group(&0x8000_0000_0000_0000u64.to_be_bytes()),
        Err(DecodeError::BadLength)
    );
    // Truncated mid-group.
    assert_eq!(
        decode_group(&good[..good.len() - 1]),
        Err(DecodeError::Truncated)
    );
    // Bytes after a complete group.
    let mut trailing = good.clone();
    trailing.push(0);
    assert_eq!(decode_group(&trailing), Err(DecodeError::TrailingBytes));
    // Non-UTF-8 in the correlation id.
    let mut bad_utf8 = good.clone();
    bad_utf8[8] = 0xff;
    assert_eq!(decode_group(&bad_utf8), Err(DecodeError::BadUtf8));
}

// ───────────────────── Ed25519 checkpoints (8h) ─────────────────────────

fn seed(b: u8) -> [u8; 32] {
    [b; 32]
}

/// The whole point of 8h: verification needs the **public** key, and a
/// checkpoint signed by another key does not verify under it.
#[test]
fn checkpoint_verifies_from_the_public_key_alone_and_a_forgery_does_not() {
    use crate::sign::{CheckpointVerifier, Ed25519Signer};

    let mut store = InMemoryStore::new();
    store.append(create_group("grp-0")).unwrap();
    store.append(create_group("grp-1")).unwrap();

    let signer = Ed25519Signer::from_seed("chk-2026-08", &seed(1));
    let cp = checkpoint(&store, &space(), &signer, T9).unwrap();
    assert_eq!(cp.key_id, "chk-2026-08");
    assert_eq!(cp.signature.len(), 64, "Ed25519 signatures are 64 bytes");

    // A receiver holds ONLY the published key id and 64 hex characters.
    // Nothing here can sign.
    let published = signer.verifier().public_hex();
    let receiver = CheckpointVerifier::from_public_hex("chk-2026-08", &published).unwrap();
    assert!(receiver.verify_checkpoint(&cp));

    // A different key signing the same head does not verify for that
    // receiver — this is the forgery the stub signer admitted, since its
    // "signature" was a keyless fold anyone could recompute.
    let attacker = Ed25519Signer::from_seed("chk-2026-08", &seed(2));
    let mut forged = checkpoint(&store, &space(), &attacker, T9).unwrap();
    assert_ne!(forged.signature, cp.signature);
    assert!(!receiver.verify_checkpoint(&forged));

    // Nor does lifting the genuine signature onto a doctored claim.
    forged.signature = cp.signature.clone();
    forged.upto = 1;
    assert!(!receiver.verify_checkpoint(&forged));

    // Nor does a checkpoint that names a key the receiver does not hold,
    // even when the signature itself is valid under the key it names.
    let other_key = Ed25519Signer::from_seed("chk-2026-09", &seed(3));
    let rolled = checkpoint(&store, &space(), &other_key, T9).unwrap();
    assert!(other_key.verifier().verify_checkpoint(&rolled));
    assert!(!receiver.verify_checkpoint(&rolled));

    // Every field the signature covers is load-bearing.
    let doctors: [fn(&mut Checkpoint); 4] = [
        |c: &mut Checkpoint| c.upto = 99,
        |c: &mut Checkpoint| c.at = T0,
        |c: &mut Checkpoint| c.head = ChainHash::GENESIS,
        |c: &mut Checkpoint| c.space = other_space(),
    ];
    for doctor in doctors {
        let mut tampered = cp.clone();
        doctor(&mut tampered);
        assert!(!receiver.verify_checkpoint(&tampered));
    }
}

/// The signer's own `verify` must not be re-sign-and-compare: it routes
/// through the public key, so the signer and a third party run the same
/// check.
#[test]
fn signer_verify_agrees_with_public_verification() {
    use crate::sign::Ed25519Signer;
    use crate::store::Signer as _;
    let signer = Ed25519Signer::from_seed("k", &seed(9));
    let sig = signer.sign(b"attested head");
    assert!(signer.verify(b"attested head", &sig));
    assert!(!signer.verify(b"attested heaD", &sig));
    assert!(!signer.verify(b"attested head", &sig[..63]));
    assert!(signer.verifier().verify(b"attested head", &sig));
}

#[test]
fn published_key_material_is_refused_when_malformed() {
    use crate::sign::{CheckpointVerifier, SignerError};
    assert!(matches!(
        CheckpointVerifier::from_public_hex("k", "beef"),
        Err(SignerError::BadKey(_))
    ));
    assert!(matches!(
        CheckpointVerifier::from_public_hex("k", &"zz".repeat(32)),
        Err(SignerError::BadKey(_))
    ));
}

// ──────────────────────── streaming verification ────────────────────────

/// `verify` batches through `entries`, so a chain longer than one batch
/// must still verify — and a tamper past the first batch must still be
/// found, at its true index.
#[test]
fn verification_streams_past_one_batch() {
    use crate::store::VERIFY_BATCH;
    let mut store = InMemoryStore::new();
    let n = VERIFY_BATCH + 7;
    for i in 0..n {
        store.append(create_group(&format!("grp-{i}"))).unwrap();
    }
    assert_eq!(store.len(&space()), n);
    assert!(store.verify(&space()).is_ok());
    assert!(Store::verify_from(&store, &space(), ChainHash::GENESIS, 0).is_ok());

    let victim = VERIFY_BATCH + 3;
    store.chain_mut(&space())[victim].group.commands[0].row_id = "txn_swapped".into();
    let err = Store::verify_from(&store, &space(), ChainHash::GENESIS, 0).unwrap_err();
    assert_eq!(err.index, victim);
    assert_eq!(err.kind, TamperKind::HashMismatch);
}

// ───────────── the restored-unverified flag (plan-opslog.md D7) ─────────

/// **The second backward-compatibility proof, pinned the same way the first
/// one was.**
///
/// `f4d7bdac…` is the chain hash of `create_group("grp-d7")` at genesis,
/// and `165` is its byte length, both computed and written down **before**
/// `restored_unverified` existed on [`CommandGroup`], against the encoder
/// that had only ever heard of `owner_administration`'s trailer. This group
/// carries neither flag, so if adding the second trailer tag moved a single
/// byte of its rendering, this constant would stop matching and every
/// chain, checkpoint and signature ever written over a group with no
/// trailer at all would be void.
///
/// Pinned rather than derived, for the reason `PRE_FLAG_GOLDEN` states: a
/// golden computed against the changed code proves nothing about the code
/// it was meant to check.
const PRE_SECOND_FLAG_GOLDEN: &str =
    "f4d7bdac1c78635a7cee74790d223e87e998909865e30a14f99afa4f543932e5";

/// 165 bytes, two more than [`PRE_FLAG_LEN`], because `owner_administration`
/// already exists when this golden is taken; `create_group` itself supplies
/// no trailer byte for either flag.
const PRE_SECOND_FLAG_LEN: usize = 165;

#[test]
fn an_unflagged_group_still_hashes_where_it_did() {
    let group = create_group("grp-d7");
    assert!(!group.owner_administration);
    let bytes = crate::chain::encode_group(&group);
    assert_eq!(
        bytes.len(),
        PRE_SECOND_FLAG_LEN,
        "a group flagged neither way must render the bytes it rendered \
         before the second trailer's field existed, not one more"
    );
    assert_eq!(
        crate::chain::hash_group(ChainHash::GENESIS, &group).to_string(),
        PRE_SECOND_FLAG_GOLDEN,
        "adding restored_unverified moved an existing group's hash; every \
         chain already written is now unverifiable"
    );
}

// ───────────── the restore marker (`plan-production-ops.md` R2a) ─────────────

/// **The third backward-compatibility proof, pinned the way the first two
/// were.**
///
/// This is the chain hash of `create_group("grp-r2a")` at genesis, and
/// [`PRE_THIRD_FLAG_LEN`] its byte length, both computed and committed
/// **before** `CommandGroup::restore` existed, against the encoder that
/// knew two trailer tags and no payload. The group carries no trailer, so
/// if adding the restore trailer (`0x03`, the first with a payload) moved a
/// single byte of its rendering, this constant stops matching and every
/// chain, checkpoint and signature over an ordinary group is void.
///
/// Pinned rather than derived, for the reason `PRE_FLAG_GOLDEN` states.
const PRE_THIRD_FLAG_GOLDEN: &str =
    "36b12dcf0852608a8a50014eb258bbaa3f596a22e04a4d26b4bb28832e774356";

/// The group's rendering before the third tag existed, measured then: two
/// bytes more than [`PRE_SECOND_FLAG_LEN`] because `grp-r2a` is one byte
/// longer than `grp-d7` and appears twice (the correlation and the command
/// id `create_group` derives from it); no trailer byte.
const PRE_THIRD_FLAG_LEN: usize = 167;

/// The companion pin for the two tags that already existed: a group carrying
/// both `0x01` and `0x02`, hashed before the restore trailer existed. The
/// unflagged golden proves silence renders as it did; this proves the two
/// older tags, together, still do, so every revert and every rule-6 group
/// already written keeps its hash too.
const PRE_THIRD_FLAG_BOTH_TAGS_GOLDEN: &str =
    "009fcb9bd6b4076ff4633ab717728c5659579d9657c48a662d875a6b4a14d0d3";

fn both_tags_group() -> CommandGroup {
    let mut group = create_group("grp-r2a-both");
    group.owner_administration = true;
    group.restored_unverified = true;
    group
}

#[test]
fn an_unflagged_group_hashes_where_it_did_before_the_restore_trailer() {
    let group = create_group("grp-r2a");
    let bytes = crate::chain::encode_group(&group);
    assert_eq!(
        bytes.len(),
        PRE_THIRD_FLAG_LEN,
        "a group with no trailer must render the bytes it rendered before \
         the restore trailer existed, not one more"
    );
    assert_eq!(
        crate::chain::hash_group(ChainHash::GENESIS, &group).to_string(),
        PRE_THIRD_FLAG_GOLDEN,
        "adding the restore marker moved an existing group's hash; every \
         chain already written is now unverifiable"
    );
    // And the old bytes are a lawful current encoding: they decode, as a
    // group carrying no marker, and re-encode to themselves.
    let decoded = crate::chain::decode_group(&bytes).expect("decodes");
    assert_eq!(decoded, group);
    assert_eq!(crate::chain::encode_group(&decoded), bytes);
}

#[test]
fn a_group_with_both_older_tags_hashes_where_it_did_before_the_restore_trailer() {
    let group = both_tags_group();
    let bytes = crate::chain::encode_group(&group);
    assert_eq!(&bytes[bytes.len() - 2..], &[0x01, 0x02]);
    assert_eq!(
        crate::chain::hash_group(ChainHash::GENESIS, &group).to_string(),
        PRE_THIRD_FLAG_BOTH_TAGS_GOLDEN,
        "adding the restore marker moved a flagged group's hash"
    );
    let decoded = crate::chain::decode_group(&bytes).expect("decodes");
    assert_eq!(decoded, group);
    assert_eq!(crate::chain::encode_group(&decoded), bytes);
}

fn marker(source: crate::command::RestoreSource) -> crate::command::RestoreMarker {
    crate::command::RestoreMarker {
        space: space(),
        restored_from: (41, ChainHash([0xab; 32])),
        pre_restore_head: Some((57, ChainHash([0xcd; 32]))),
        source,
        operator: "ops@example.test".into(),
    }
}

fn marker_group(correlation: &str) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(correlation.into()),
        actor: Actor::System("restore".into()),
        commands: vec![],
        owner_administration: false,
        restored_unverified: false,
        restore: Some(marker(crate::command::RestoreSource::Bundle {
            manifest_sha256: [0x5a; 32],
        })),
    }
}

/// The encoded length of `group` with its marker removed: where the `0x03`
/// tag sits in the marked rendering.
fn marker_tag_at(group: &CommandGroup) -> usize {
    let mut unmarked = group.clone();
    unmarked.restore = None;
    crate::chain::encode_group(&unmarked).len()
}

#[test]
fn a_restore_marker_round_trips() {
    use crate::chain::{decode_group, encode_group, hash_group};
    use crate::command::{RestoreMarker, RestoreSource};
    let bundle = marker_group("grp-restore");
    let mut pitr = marker_group("grp-restore-pitr");
    pitr.restore = Some(RestoreMarker {
        pre_restore_head: None,
        ..marker(RestoreSource::Pitr {
            at: Timestamp(1_757_000_000),
        })
    });
    for group in [bundle, pitr] {
        let bytes = encode_group(&group);
        assert_eq!(bytes[marker_tag_at(&group)], 0x03);
        assert_eq!(decode_group(&bytes), Ok(group.clone()));
        assert_eq!(encode_group(&decode_group(&bytes).unwrap()), bytes);
        let mut unmarked = group.clone();
        unmarked.restore = None;
        assert_ne!(
            hash_group(ChainHash::GENESIS, &group),
            hash_group(ChainHash::GENESIS, &unmarked),
            "the marker is inside the hash"
        );
    }
    // Every field of the payload is covered: change any one and the hash
    // moves.
    let base = marker_group("grp-restore");
    let h = hash_group(ChainHash::GENESIS, &base);
    let edits: [fn(&mut RestoreMarker); 5] = [
        |m| m.space = SpaceId("spc_other".into()),
        |m| m.restored_from.0 += 1,
        |m| m.pre_restore_head = None,
        |m| {
            m.source = RestoreSource::Bundle {
                manifest_sha256: [0x5b; 32],
            }
        },
        |m| m.operator.push('x'),
    ];
    for edit in edits {
        let mut g = base.clone();
        edit(g.restore.as_mut().unwrap());
        assert_ne!(hash_group(ChainHash::GENESIS, &g), h);
    }
}

#[test]
fn a_restore_marker_with_commands_is_refused() {
    let mut group = marker_group("grp-restore-writes");
    group.commands = create_group("grp-x").commands;
    assert_eq!(
        group.validate(),
        Err(GroupShapeError::MarkerWithCommands {
            correlation: CorrelationId("grp-restore-writes".into())
        })
    );
    let mut store = InMemoryStore::new();
    assert!(matches!(
        store.append(group),
        Err(AppendError::Shape(
            GroupShapeError::MarkerWithCommands { .. }
        ))
    ));
    // The empty marker is admitted, in the marker's own space.
    let ok = marker_group("grp-restore-ok");
    assert_eq!(ok.validate(), Ok(&space()));
    assert!(store.append(ok).is_ok());
}

#[test]
fn a_restore_marker_under_another_actor_is_refused() {
    let mut group = marker_group("grp-restore-actor");
    group.actor = Actor::User("usr_x".into());
    assert_eq!(
        group.validate(),
        Err(GroupShapeError::MarkerActor {
            correlation: CorrelationId("grp-restore-actor".into()),
            actor: "user:usr_x".into(),
        })
    );
    group.actor = Actor::System("retention".into());
    assert!(matches!(
        group.validate(),
        Err(GroupShapeError::MarkerActor { .. })
    ));
}

#[test]
fn an_empty_group_without_a_marker_is_still_refused() {
    let mut empty = marker_group("grp-empty-no-marker");
    empty.restore = None;
    assert!(matches!(
        empty.validate(),
        Err(GroupShapeError::EmptyGroup { .. })
    ));
    let mut store = InMemoryStore::new();
    assert!(matches!(
        store.append(empty),
        Err(AppendError::Shape(GroupShapeError::EmptyGroup { .. }))
    ));
}

#[test]
fn a_truncated_marker_payload_refuses() {
    use crate::chain::{decode_group, encode_group, DecodeError};
    let group = marker_group("grp-restore-cut");
    let bytes = encode_group(&group);
    let payload_start = marker_tag_at(&group) + 1;
    let space_len = 8 + space().0.len();
    // Three cuts: inside the space string, inside `restored_from`'s hash,
    // and one byte short of the operator's end. A short read fails as the
    // reader fails anywhere else: `Truncated`, or `BadLength` when a
    // string's length prefix outruns what is left.
    for cut in [
        payload_start + 10,
        payload_start + space_len + 8 + 16,
        bytes.len() - 1,
    ] {
        let got = decode_group(&bytes[..cut]);
        assert!(
            matches!(got, Err(DecodeError::Truncated | DecodeError::BadLength)),
            "a payload cut at {cut} of {} must refuse, got {got:?}",
            bytes.len()
        );
    }
    // The bare tag with no payload at all refuses too.
    assert_eq!(
        decode_group(&bytes[..payload_start]),
        Err(DecodeError::Truncated)
    );
}

#[test]
fn the_restore_tag_before_a_lower_tag_refuses() {
    use crate::chain::{decode_group, encode_group, DecodeError};
    let group = marker_group("grp-restore-order");
    let one = encode_group(&group);

    // `0x03 ‖ payload ‖ 0x01`: a lower tag after the highest one.
    let mut lower_after = one.clone();
    lower_after.push(0x01);
    assert_eq!(decode_group(&lower_after), Err(DecodeError::TrailingBytes));

    // The marker stated twice.
    let payload = one[marker_tag_at(&group)..].to_vec();
    let mut twice = one.clone();
    twice.extend_from_slice(&payload);
    assert_eq!(decode_group(&twice), Err(DecodeError::TrailingBytes));

    // And the payload's own option byte is closed: `pre_restore_head` is
    // `0x00` or `0x01`, nothing else.
    let head_flag_at = marker_tag_at(&group) + 1 + 8 + space().0.len() + 8 + 32;
    assert_eq!(one[head_flag_at], 0x01);
    let mut bad = one.clone();
    bad[head_flag_at] = 0x02;
    assert_eq!(decode_group(&bad), Err(DecodeError::BadTag(0x02)));
}

#[test]
fn a_marker_group_renders_all_three_tags_in_order() {
    use crate::chain::{decode_group, encode_group};
    let mut group = marker_group("grp-restore-all");
    group.owner_administration = true;
    group.restored_unverified = true;
    let bytes = encode_group(&group);
    let mut bare = group.clone();
    bare.owner_administration = false;
    bare.restored_unverified = false;
    bare.restore = None;
    let at = encode_group(&bare).len();
    assert_eq!(&bytes[at..at + 3], &[0x01, 0x02, 0x03]);
    assert_eq!(decode_group(&bytes), Ok(group));
}

#[test]
fn a_marker_cannot_be_reverted() {
    let mut store = InMemoryStore::new();
    store
        .append(marker_group("grp-restore-rev"))
        .expect("a marker appends");
    let got = revert_group(
        &store,
        &space(),
        &CorrelationId("grp-restore-rev".into()),
        &reverter(),
        T9,
    );
    assert_eq!(
        got,
        Err(RevertError::RestoreMarker(CorrelationId(
            "grp-restore-rev".into()
        )))
    );
    assert!(got.unwrap_err().to_string().contains("nothing to undo"));
}

// ───────────────────────── the decimal value tag (27ak) ──────────────────

/// A group whose images carry **every value tag that existed before the
/// decimal tag**: null, bool, int, text, id and a sealed handle, in a
/// converge's before and after images.
fn every_old_tag_group() -> CommandGroup {
    let image = img(&[
        ("a_null", Value::Null),
        ("b_bool", Value::Bool(true)),
        ("c_int", Value::Int(-2999)),
        ("d_text", Value::Text("29.99".into())),
        ("e_id", Value::Id("itm_01".into())),
        ("f_sealed", Value::SealedRef([0x5a; 16])),
    ]);
    CommandGroup {
        correlation: CorrelationId("grp-27ak".into()),
        actor: Actor::User("usr_x".into()),
        commands: vec![cmd(
            "cmd-grp-27ak",
            CommandKind::Converge,
            Some(image.clone()),
            Some(image),
        )],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

/// The hash of [`every_old_tag_group`] at genesis, **computed before the
/// decimal tag existed** (27ak, 2026-10-04).
///
/// The decimal tag is added by presence: a new tag byte, `6`, that only a
/// decimal value renders. So every group written before it, which holds no
/// decimal, renders the bytes it always rendered. If adding the tag moved a
/// byte of any older value's rendering, this stops matching and every chain,
/// checkpoint and signature already written is void.
const PRE_DECIMAL_TAG_GOLDEN: &str =
    "071f5908ce9c3f32310a9caf178f02d43c80e0d09ec2b84efefbabc29e833cb0";

#[test]
fn every_older_value_tag_hashes_where_it_did_before_the_decimal_tag() {
    let group = every_old_tag_group();
    assert_eq!(
        crate::chain::hash_group(ChainHash::GENESIS, &group).to_string(),
        PRE_DECIMAL_TAG_GOLDEN,
        "adding the decimal tag moved an existing value's rendering; every \
         chain already written is now unverifiable"
    );
    let bytes = crate::chain::encode_group(&group);
    let decoded = crate::chain::decode_group(&bytes).expect("decodes");
    assert_eq!(decoded, group);
    assert_eq!(crate::chain::encode_group(&decoded), bytes);
}

/// One spelling per value: trailing zeros stripped, no leading zeros, no
/// `+`, no `-0`.
#[test]
fn a_decimal_has_one_canonical_spelling() {
    use crate::value::Decimal;
    let canon = |s: &str| Decimal::parse(s).map(|d| d.to_string());
    assert_eq!(canon("0.0800").as_deref(), Some("0.08"));
    assert_eq!(canon("0.08").as_deref(), Some("0.08"));
    assert_eq!(canon("+0.08").as_deref(), Some("0.08"));
    assert_eq!(canon("007.50").as_deref(), Some("7.5"));
    assert_eq!(canon("-0.000").as_deref(), Some("0"));
    assert_eq!(canon("-12.50").as_deref(), Some("-12.5"));
    assert_eq!(canon("100").as_deref(), Some("100"));
    assert_eq!(canon("1."), None);
    assert_eq!(canon(".5"), None);
    assert_eq!(canon("1e3"), None);
    assert_eq!(canon(""), None);
}

fn decimal_group(spelling: &str) -> CommandGroup {
    let mut group = create_group("grp-27ak-dec");
    group.commands[0].after = Some(img(&[(
        "rate",
        Value::Decimal(crate::value::Decimal::parse(spelling).expect("a decimal")),
    )]));
    group
}

/// The decimal tag is byte 6, its payload the canonical string, and it
/// round-trips. A value written at two scales hashes once.
#[test]
fn a_decimal_value_rides_tag_six_and_hashes_by_value() {
    let group = decimal_group("0.0800");
    let bytes = crate::chain::encode_group(&group);
    let needle = [&[6u8][..], &4u64.to_be_bytes()[..], b"0.08"].concat();
    assert!(
        bytes.windows(needle.len()).any(|w| w == needle.as_slice()),
        "tag 6, then the canonical spelling as a length-prefixed string"
    );
    let decoded = crate::chain::decode_group(&bytes).expect("decodes");
    assert_eq!(decoded, group);
    assert_eq!(crate::chain::encode_group(&decoded), bytes);
    assert_eq!(
        crate::chain::hash_group(ChainHash::GENESIS, &decimal_group("0.0800")),
        crate::chain::hash_group(ChainHash::GENESIS, &decimal_group("0.08")),
        "0.0800 at scale 4 and 0.08 at scale 2 are one value"
    );
}

/// A stored spelling that is not canonical is not something the encoder
/// ever wrote, so it is refused rather than decoded into a second rendering.
#[test]
fn a_non_canonical_decimal_spelling_does_not_decode() {
    let bytes = crate::chain::encode_group(&decimal_group("0.08"));
    let needle = [&[6u8][..], &4u64.to_be_bytes()[..], b"0.08"].concat();
    let at = bytes
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .expect("the decimal payload");
    // Same length, a different spelling of a different string: `00.8`
    // is not canonical (a leading zero).
    let mut forged = bytes.clone();
    forged[at + 9..at + 13].copy_from_slice(b"00.8");
    assert!(matches!(
        crate::chain::decode_group(&forged),
        Err(crate::chain::DecodeError::BadDecimal(_))
    ));
}
