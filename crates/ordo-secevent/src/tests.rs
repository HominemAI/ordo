//! The stream's own rules, proved without a database. The durable half —
//! survival across a domain revert, independent verification against a
//! separate key, and the assertion that a security append never touches the
//! command chain's lock — is in `ordo-exec/tests/secevent.rs`, which needs
//! the conformance cluster.

use std::collections::BTreeSet;

use ordo_commandlog::{
    Actor, ChainHash, CheckpointVerifier, CorrelationId, Ed25519Signer, Signer, SpaceId,
    TamperKind, Timestamp,
};
use ordo_identity::principal::{AccessProfile, PrincipalId, Role};
use ordo_identity::{AccessEngine, CapabilityGrant, DenyReason, ManifestHash, TokenClaims};

use crate::alert::{correlations_for, DenyVolumeMonitor, DenyVolumeRule, DENIALS_METRIC};
use crate::chain::{
    decode_entry, encode_entry, hash_entry, verify_stream, ChainedEntry, DecodeError, DOMAIN,
};
use crate::collapse::Collapser;
use crate::correlation;
use crate::event::{
    EventKind, RecordedDecision, Scope, StreamEntry, Subject, SubjectKind, TokenFacts,
    OP_UNPRINTABLE,
};
use crate::recorder::{relationship, route, Observation, Recorder, Relationship, Stamp};
use crate::store::{
    checkpoint, prune_to_checkpoint, InMemorySecurityStore, Retention, SecurityStore, StreamError,
};

fn signer(id: &str, byte: u8) -> Ed25519Signer {
    Ed25519Signer::from_seed(id, &[byte; 32])
}

fn space() -> SpaceId {
    SpaceId("spc_alpha".into())
}

fn cor(n: u32) -> CorrelationId {
    CorrelationId(format!("cor_{n:08x}"))
}

fn deny_entry(n: u32, decision: RecordedDecision) -> StreamEntry {
    StreamEntry::single(
        Scope::Space(space()),
        Subject::member("usr_1"),
        "openTasks",
        EventKind::denied(decision),
        Timestamp(1_000 + u64::from(n)),
        cor(n),
    )
}

// ───────────────────────── encoding & chain ─────────────────────────────

#[test]
fn the_encoding_is_a_decoding() {
    let entries = vec![
        deny_entry(1, RecordedDecision::NoMembership),
        StreamEntry {
            scope: Scope::Deployment,
            about: Some(SpaceId("spc_probed".into())),
            subject: Subject::Unverified,
            op: "probe.name".into(),
            kind: EventKind::Denied(BTreeSet::from([
                RecordedDecision::TokenUnverifiable,
                RecordedDecision::UnregisteredOp,
            ])),
            first_at: Timestamp(10),
            last_at: Timestamp(70),
            count: 412,
            correlations: vec![cor(1), cor(2)],
        },
        StreamEntry::single(
            Scope::Deployment,
            Subject::platform("custody"),
            "",
            EventKind::KeyReleased {
                purpose: "reseal".into(),
            },
            Timestamp(11),
            cor(3),
        ),
        StreamEntry::single(
            Scope::Space(space()),
            Subject::capability("agt_bot"),
            "listOrders",
            EventKind::AuthenticatorLinked {
                added: "aut_new".into(),
                vouched_by: Some("aut_old".into()),
            },
            Timestamp(12),
            cor(4),
        ),
        StreamEntry::single(
            Scope::Space(space()),
            Subject::platform("secevent"),
            "",
            EventKind::Suppressed { further: 9 },
            Timestamp(13),
            cor(5),
        ),
        StreamEntry::single(
            Scope::Space(space()),
            Subject::platform("secevent"),
            "",
            EventKind::EventsDropped { lost: 4_102 },
            Timestamp(14),
            cor(6),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::member("usr_admin"),
            "enrol.ticket-issued",
            EventKind::Enrolment {
                act: "ticket-issued".into(),
                detail: "purpose=password ticket=ab12cd34 for=usr_ana".into(),
            },
            Timestamp(15),
            cor(7),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::member("usr_ana"),
            "auth.logout-all",
            EventKind::SessionsEnded {
                reason: "logout-all".into(),
            },
            Timestamp(16),
            cor(8),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::platform("recovery"),
            "recovery.completed",
            EventKind::Recovery {
                act: "completed".into(),
                detail: "case=rec_1 principal=usr_ana path=host".into(),
            },
            Timestamp(17),
            cor(9),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::member("usr_ana"),
            "auth.second-step",
            EventKind::SecondFactor {
                act: "disabled".into(),
                detail: "principal=usr_ana kind=totp".into(),
            },
            Timestamp(18),
            cor(10),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::member("usr_ana"),
            "auth.handoff.redeem",
            EventKind::SessionHandoff {
                act: "redeemed".into(),
                detail: "principal=usr_ana origin=https://app.example.com".into(),
            },
            Timestamp(19),
            cor(11),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::member("usr_ana"),
            "auth.complete",
            EventKind::UpstreamClaim {
                act: "presence-vouched".into(),
                detail: "principal=usr_ana connection=corp amr=pwd,mfa auth_time=19".into(),
            },
            Timestamp(20),
            cor(12),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::platform("keyprovider"),
            "",
            EventKind::SealedUnreadable {
                entity: "orders".into(),
                field: "memo".into(),
                epoch: 2,
            },
            Timestamp(21),
            cor(13),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::member("usr_ana"),
            "std.deployment.addConsoleKey",
            EventKind::ConsoleKey {
                act: "added".into(),
                detail: "key=ck_1 principal=usr_ana name=console".into(),
            },
            Timestamp(22),
            cor(14),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::platform("boot"),
            "deployment.boot",
            EventKind::DeploymentBooted {
                detail: "revision=3 applied_by=owner:o1 fingerprint=ab".into(),
            },
            Timestamp(23),
            cor(15),
        ),
        StreamEntry::single(
            Scope::Deployment,
            Subject::member("usr_ana"),
            "std.deployment.activate",
            EventKind::Deploy {
                act: "activated".into(),
                detail: "version=v1 was=v2 by=usr_ana".into(),
            },
            Timestamp(24),
            cor(16),
        ),
    ];
    for e in entries {
        let bytes = encode_entry(&e);
        assert_eq!(decode_entry(&bytes).expect("round trips"), e);
    }
}

/// The two markers are two markers.
///
/// `Suppressed` is the cap refusing to grow the record with an attacker's
/// variety; `EventsDropped` is the record admitting a storage fault. They
/// encode differently, they render differently, and a reader can tell them
/// apart — which is the whole reason the second one is not folded into the
/// first.
#[test]
fn suppression_and_loss_are_distinguishable_in_the_record() {
    let marker = |kind: EventKind| {
        StreamEntry::single(
            Scope::Deployment,
            Subject::platform("secevent"),
            "",
            kind,
            Timestamp(1),
            cor(1),
        )
    };
    let capped = marker(EventKind::Suppressed { further: 7 });
    let lost = marker(EventKind::EventsDropped { lost: 7 });
    assert_ne!(
        encode_entry(&capped),
        encode_entry(&lost),
        "two different admissions must not hash to the same bytes"
    );
    assert!(capped.render().contains("suppressed"));
    assert!(
        lost.render().contains("unreachable"),
        "the loss marker must say WHY, or an operator reads it as a cap \
         firing: {}",
        lost.render()
    );
    assert!(capped.kind.is_marker() && lost.kind.is_marker());
}

#[test]
fn a_command_group_body_is_not_a_security_body() {
    // Domain separation, checked rather than asserted in prose: the exact
    // bytes the command chain hashes must not decode here. Without the
    // domain prefix a stored group could be replayed into the security
    // table and would verify as *something*.
    let group = ordo_commandlog::CommandGroup {
        correlation: ordo_commandlog::CorrelationId("cg_1".into()),
        actor: Actor::User("usr_1".into()),
        commands: vec![ordo_commandlog::Command {
            id: ordo_commandlog::CommandId("cmd_1".into()),
            space: space(),
            entity: "task".into(),
            row_id: "tsk_1".into(),
            kind: ordo_commandlog::CommandKind::Create,
            before: None,
            after: Some(ordo_commandlog::Image::new()),
            at: Timestamp(1),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    };
    let bytes = ordo_commandlog::encode_group(&group);
    assert_eq!(decode_entry(&bytes), Err(DecodeError::WrongDomain));
    assert!(encode_entry(&deny_entry(1, RecordedDecision::NoMembership)).starts_with(DOMAIN));
}

#[test]
fn an_altered_entry_is_caught_at_its_index() {
    let mut store = InMemorySecurityStore::new();
    let scope = Scope::Space(space());
    for n in 0..5 {
        store
            .append(deny_entry(n, RecordedDecision::OpNotInRole))
            .expect("appends");
    }
    assert!(store.verify(&scope).is_ok());

    // Rewrite the third entry's content, leaving its stored hash alone.
    store.stream_mut(&scope)[2].entry.count = 9_999;
    let bad = store.verify(&scope).expect_err("tamper is caught");
    assert_eq!(bad.index, 2);
    assert_eq!(bad.kind, TamperKind::HashMismatch);

    // And a broken link is caught before content.
    let mut store2 = InMemorySecurityStore::new();
    for n in 0..3 {
        store2
            .append(deny_entry(n, RecordedDecision::OpNotInRole))
            .expect("appends");
    }
    store2.stream_mut(&scope)[1].prev = ChainHash::GENESIS;
    let bad = store2.verify(&scope).expect_err("broken link is caught");
    assert_eq!((bad.index, bad.kind), (1, TamperKind::BrokenLink));
}

#[test]
fn the_deployment_stream_is_its_own_chain() {
    let mut store = InMemorySecurityStore::new();
    store
        .append(deny_entry(1, RecordedDecision::NoMembership))
        .expect("space entry");
    let dep = StreamEntry::single(
        Scope::Deployment,
        Subject::Unverified,
        "openTasks",
        EventKind::denied(RecordedDecision::TokenUnverifiable),
        Timestamp(5),
        cor(2),
    );
    let stored = store.append(dep).expect("deployment entry");
    // A separate chain: the deployment entry is a genesis link, not a
    // continuation of the space's.
    assert_eq!(stored.prev, ChainHash::GENESIS);
    assert!(store.verify(&Scope::Deployment).is_ok());
    assert!(store.verify(&Scope::Space(space())).is_ok());
    assert_eq!(crate::event::DEPLOYMENT_KEY, Scope::Deployment.key().0);
}

// ───────────────────────── separately keyed ─────────────────────────────

#[test]
fn a_security_checkpoint_does_not_verify_under_the_command_chain_key() {
    let mut store = InMemorySecurityStore::new();
    let scope = Scope::Space(space());
    for n in 0..4 {
        store
            .append(deny_entry(n, RecordedDecision::NoMembership))
            .expect("appends");
    }
    let sec = signer("sec-2026", 7);
    let chain = signer("chain-2026", 9);
    let cp = checkpoint(&store, &scope, &sec, Timestamp(9_000)).expect("head exists");

    assert!(cp.verify(&sec.verifier()), "its own key verifies it");
    assert!(
        !cp.verify(&chain.verifier()),
        "the command chain's key must not"
    );

    // Even under one key, the two checkpoint families are different
    // messages: the domain tag is inside the signed bytes.
    let same = signer("shared", 3);
    let cmd_bytes =
        ordo_commandlog::Checkpoint::signed_bytes(&scope.key(), cp.upto, cp.head, Timestamp(9_000));
    let sec_bytes =
        crate::store::StreamCheckpoint::signed_bytes(&scope, cp.upto, cp.head, Timestamp(9_000));
    assert_ne!(cmd_bytes, sec_bytes);
    let cmd_sig = same.sign(&cmd_bytes);
    let cross = crate::store::StreamCheckpoint {
        signature: cmd_sig,
        key_id: "shared".into(),
        ..cp.clone()
    };
    assert!(
        !cross.verify(&same.verifier()),
        "a command-chain signature must not verify as a stream checkpoint"
    );
}

#[test]
fn key_discovery_is_part_of_verification() {
    let mut store = InMemorySecurityStore::new();
    let scope = Scope::Space(space());
    store
        .append(deny_entry(1, RecordedDecision::NoMembership))
        .expect("appends");
    let sec = signer("sec-2026", 7);
    let cp = checkpoint(&store, &scope, &sec, Timestamp(1)).expect("head");
    // Same public bytes, different advertised id: refused, so a receiver
    // that trusts id B cannot be handed A's key and accept A's marker.
    let mislabelled =
        CheckpointVerifier::from_public_hex("other-id", &sec.verifier().public_hex()).expect("key");
    assert!(!cp.verify(&mislabelled));
}

/// **The two signing paths produce the same bytes.**
///
/// [`checkpoint`] is the synchronous path over a `SecurityStore`;
/// `checkpoint_at` is the one `ordo_exec::secevent::checkpoint_scope` uses,
/// because a trait whose methods block cannot be driven from inside a
/// runtime. That split is the same one `check_prune` already makes, and it
/// carries the same hazard: two definitions of what a stream checkpoint's
/// signed bytes *are* would drift, and the day they did the deployment would
/// be signing markers its own verifier rejects. So the sync path delegates,
/// and this is the assertion that it does.
#[test]
fn both_signing_paths_produce_the_same_marker() {
    let mut store = InMemorySecurityStore::new();
    let scope = Scope::Space(space());
    for n in 0..3 {
        store
            .append(deny_entry(n, RecordedDecision::NoMembership))
            .expect("appends");
    }
    let sec = signer("sec-2026", 7);
    let over_store = checkpoint(&store, &scope, &sec, Timestamp(4_000)).expect("head");
    let by_hand = crate::store::checkpoint_at(
        &scope,
        3,
        store.head(&scope).expect("head"),
        &sec,
        Timestamp(4_000),
    );
    assert_eq!(over_store, by_hand);
    assert!(by_hand.verify(&sec.verifier()));
}

// ─────────────────────── separately retained ────────────────────────────

#[test]
fn retention_drops_a_whole_prefix_and_verification_resumes() {
    let mut store = InMemorySecurityStore::new();
    let scope = Scope::Space(space());
    for n in 0..10 {
        store
            .append(deny_entry(n, RecordedDecision::NoMembership))
            .expect("appends");
    }
    let sec = signer("sec-2026", 7);
    // A checkpoint over the first six entries, taken while all ten are
    // stored: `upto` is a position in the full stream, not a count of what
    // is currently retained.
    let head6 = store.entries(&scope, 5, 1)[0].hash;
    let cp = crate::store::StreamCheckpoint {
        scope: scope.clone(),
        upto: 6,
        head: head6,
        at: Timestamp(2_000),
        key_id: sec.key_id().to_string(),
        signature: sec.sign(&crate::store::StreamCheckpoint::signed_bytes(
            &scope,
            6,
            head6,
            Timestamp(2_000),
        )),
    };

    let anchor = prune_to_checkpoint(&mut store, &cp, &sec.verifier()).expect("lawful prune");
    assert_eq!(store.len(&scope), 4);
    assert_eq!(store.base_index(&scope), 6);
    assert!(
        store.verify_from(&scope, anchor, 6).is_ok(),
        "the retained suffix verifies from the checkpoint it was cut at"
    );
    // And verifying a pruned stream as if it started at genesis is refused
    // rather than reported as tampering at index 0.
    assert_eq!(
        store.verify(&scope).expect_err("pruned").kind,
        TamperKind::Unreadable
    );

    // A prune the checkpoint does not cover is refused.
    let mut store2 = InMemorySecurityStore::new();
    for n in 0..3 {
        store2
            .append(deny_entry(n, RecordedDecision::NoMembership))
            .expect("appends");
    }
    let forged = crate::store::StreamCheckpoint {
        signature: vec![0u8; 64],
        ..cp.clone()
    };
    assert_eq!(
        prune_to_checkpoint(&mut store2, &forged, &sec.verifier()),
        Err(StreamError::BadCheckpoint),
        "pruning on an unverified checkpoint is deleting evidence on request"
    );
}

#[test]
fn retention_is_its_own_number() {
    let mut store = InMemorySecurityStore::new();
    let scope = Scope::Space(space());
    for n in 0..6 {
        let mut e = deny_entry(n, RecordedDecision::NoMembership);
        e.first_at = Timestamp(u64::from(n) * 100);
        e.last_at = e.first_at;
        store.append(e).expect("appends");
    }
    // Keep 250s: entries at 0, 100 fall outside a now of 400 (cutoff 150).
    let policy = Retention {
        keep_secs: 250,
        deployment_keep_secs: 250,
    };
    assert_eq!(policy.prunable_upto(&store, &scope, Timestamp(400)), 2);
    // A longer retention prunes nothing, and nothing about the command
    // chain's policy is consulted to decide either answer.
    assert_eq!(
        Retention::days(30).prunable_upto(&store, &scope, Timestamp(400)),
        0
    );
    // The tail bound, in the policy: every entry has aged out at a far
    // enough `now`, and the floor still stops one short of the end.
    assert_eq!(
        policy.prunable_upto(&store, &scope, Timestamp(1_000_000)),
        5,
        "the floor never reaches the last entry"
    );
}

/// Retention must not be able to empty a stream, and the reason is not
/// only that an empty stream is ambiguous to verify.
///
/// `ordo_exec::secevent`'s append reads the head out of storage and
/// continues from it; with no row it restarts at index 0 from genesis. So
/// a full prune would silently **fork the chain** and leave every stored
/// checkpoint describing a history nothing holds — retention manufacturing
/// the exact shape tampering produces.
#[test]
fn retention_refuses_to_prune_a_stream_to_nothing() {
    let mut store = InMemorySecurityStore::new();
    let scope = Scope::Space(space());
    for n in 0..3 {
        store
            .append(deny_entry(n, RecordedDecision::NoMembership))
            .expect("appends");
    }
    let sec = signer("sec", 7);
    let cp = checkpoint(&store, &scope, &sec, Timestamp(9_000)).expect("head exists");
    assert_eq!(cp.upto, 3, "a checkpoint covers the whole stream");

    assert_eq!(
        prune_to_checkpoint(&mut store, &cp, &sec.verifier()),
        Err(StreamError::RetainedTail { upto: 3 }),
        "a cut that empties the stream is refused, however well signed"
    );
    assert_eq!(store.len(&scope), 3, "and nothing was removed");
    assert!(
        StreamError::RetainedTail { upto: 3 }
            .to_string()
            .contains("empty the stream"),
        "the refusal has to say what it refused"
    );
}

/// A failed append does not lose its batch, and past the bound the loss is
/// **declared** rather than dropped.
///
/// The unit-level half of the durability story; the drainer against a real
/// database is `ordo-host/tests/secdrain.rs`.
#[test]
fn a_failed_append_returns_its_entries_and_a_full_outbox_declares_the_loss() {
    let recorder = Recorder::with_outbox(Collapser::new(0, 64), 2);
    let obs = |n: u32| Observation {
        space: Some(space()),
        subject: Subject::member(&format!("usr_{n}")),
        op: "openTasks".into(),
        kind: EventKind::Authenticated,
        at: Timestamp(1_000),
        correlation: cor(n),
    };
    for n in 0..2 {
        recorder.observe(obs(n), Relationship::Member);
    }
    assert_eq!((recorder.backlog(), recorder.dropped()), (2, 0));

    // Past the bound: the oldest survive, the arrival is refused and
    // counted. Keeping the onset of a burst is the same rule the per-window
    // cap follows.
    recorder.observe(obs(2), Relationship::Member);
    assert_eq!((recorder.backlog(), recorder.dropped()), (2, 1));

    // Drain: the declaration comes first, then the entries that survived.
    let batch = recorder.drain_ready(Timestamp(2_000));
    assert_eq!(batch[0].kind, EventKind::EventsDropped { lost: 1 });
    assert_eq!(batch[1].subject, Subject::member("usr_0"));
    assert_eq!(batch.len(), 3);

    // An append that fails hands the batch back rather than eating it.
    recorder.requeue(batch);
    assert_eq!(recorder.backlog(), 2);
    assert_eq!(
        recorder.undeclared_loss(),
        1,
        "the requeued marker folded back into the tally, so the next drain \
         regenerates it rather than losing the admission"
    );

    // And `flush_into` against a healthy store writes the lot.
    let mut store = InMemorySecurityStore::new();
    let n = recorder
        .flush_into(&mut store, Timestamp(3_000))
        .expect("a healthy store takes the batch");
    assert_eq!(n, 3);
    assert_eq!(store.len(&Scope::Space(space())), 3);
    store.verify(&Scope::Space(space())).expect("chains");
}

// ───────────────────────── routing: members only ────────────────────────

fn engine_with_member() -> (AccessEngine, TokenClaims) {
    let mut engine = AccessEngine::new();
    let ispace = ordo_identity::SpaceId(space().0.clone());
    let who = PrincipalId("usr_1".into());
    engine.set_membership(&ispace, &who, Role::Viewer);
    let profile = AccessProfile::new().allow(Role::Viewer, "openTasks");
    engine.set_profile(&ispace, profile);
    let claims = TokenClaims {
        principal: who,
        capability: None,
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };
    (engine, claims)
}

#[test]
fn a_non_member_denial_never_touches_a_spaces_stream() {
    let (engine, member) = engine_with_member();
    let stranger = TokenClaims {
        principal: PrincipalId("usr_outsider".into()),
        ..member.clone()
    };

    assert_eq!(
        relationship(&engine, &member, &space()),
        Relationship::Member
    );
    assert_eq!(
        relationship(&engine, &stranger, &space()),
        Relationship::None
    );

    let recorder = Recorder::default();
    recorder.deny(
        Some(&space()),
        Subject::member("usr_outsider"),
        "openTasks",
        RecordedDecision::NoMembership,
        &Stamp::new(Timestamp(1), cor(1)),
        Relationship::None,
    );
    recorder.deny(
        Some(&space()),
        Subject::member("usr_1"),
        "openTasks",
        RecordedDecision::OpNotInRole,
        &Stamp::new(Timestamp(1), cor(2)),
        Relationship::Member,
    );
    let entries = recorder.drain_all(Timestamp(0));
    let scopes: Vec<Scope> = entries.iter().map(|e| e.scope.clone()).collect();
    assert!(scopes.contains(&Scope::Deployment));
    assert!(scopes.contains(&Scope::Space(space())));
    assert_eq!(
        entries
            .iter()
            .filter(|e| e.scope == Scope::Space(space()))
            .count(),
        1,
        "exactly one entry reached the space, and it is the member's"
    );
}

#[test]
fn a_grant_for_another_space_is_no_relationship_with_this_one() {
    let (mut engine, base) = engine_with_member();
    let ops = BTreeSet::from(["openTasks".to_string()]);
    engine.deploy_op(
        "openTasks",
        ordo_identity::OpPin {
            manifest_hash: ManifestHash([1u8; 32]),
            unguarded: false,
            entities: Vec::new(),
        },
    );
    let other = ordo_identity::SpaceId("spc_beta".into());
    let grant: CapabilityGrant = engine
        .grant_for(
            ordo_identity::GrantId("gr_test".into()),
            &other,
            ops,
            ordo_identity::Timestamp(9_999),
        )
        .expect("grant");
    let bearer = TokenClaims {
        principal: PrincipalId("agt_bot".into()),
        capability: Some(grant),
        ..base
    };
    assert_eq!(
        relationship(&engine, &bearer, &space()),
        Relationship::None,
        "a grant scoped elsewhere must not open another space's spine"
    );
    assert_eq!(
        relationship(&engine, &bearer, &SpaceId("spc_beta".into())),
        Relationship::Grant
    );
}

#[test]
fn a_suspended_member_is_still_a_member_for_routing() {
    let (mut engine, claims) = engine_with_member();
    let ispace = ordo_identity::SpaceId(space().0.clone());
    engine.suspend(&ispace, &claims.principal);
    assert_eq!(
        relationship(&engine, &claims, &space()),
        Relationship::Member,
        "the space owner is the only person who can act on a suspension \
         denial, so it belongs in their stream"
    );
}

// ───────────────────── the unverifiable presentation ─────────────────────

#[test]
fn an_unverifiable_token_produces_an_event_that_claims_no_principal() {
    let recorder = Recorder::default();
    // The caller asserted a space and an op; neither was proven, and the
    // token did not verify at all.
    recorder.observe(
        Observation {
            space: Some(space()),
            subject: Subject::Unverified,
            op: "openTasks".into(),
            kind: EventKind::denied(RecordedDecision::TokenUnverifiable),
            at: Timestamp(42),
            correlation: cor(1),
        },
        // Even if a caller could somehow assert a relationship, an
        // unverified subject can hold none — `route` decides on the
        // subject first.
        Relationship::Member,
    );
    let entries = recorder.drain_all(Timestamp(0));
    assert_eq!(entries.len(), 1);
    let e = &entries[0];

    assert_eq!(e.subject, Subject::Unverified);
    assert!(!e.subject.is_attributed());
    assert_eq!(e.scope, Scope::Deployment, "no space may be grown by this");
    assert_eq!(
        e.kind,
        EventKind::denied(RecordedDecision::TokenUnverifiable)
    );

    // The record is checked in bytes, not in prose: nothing that looks like
    // an attribution is in the hashed body.
    let bytes = encode_entry(e);
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("user:"), "no actor string");
    assert!(!text.contains("usr_"), "no principal id");
    assert!(text.contains("openTasks"), "the attempted op survives");
    assert_eq!(e.render(), e.render());
    assert!(e.render().contains("unverified"));

    // And the route is not a matter of the caller's assertion.
    assert_eq!(
        route(Some(&space()), &Subject::Unverified, Relationship::Grant),
        Scope::Deployment
    );
}

#[test]
fn caller_chosen_op_names_are_bounded_before_they_are_hashed() {
    assert_eq!(crate::sanitize_op("orders.list"), "orders.list");
    // Newlines, quotes and control bytes are how a record gets forged by
    // the person it is about.
    assert_eq!(crate::sanitize_op("ok\ndeny usr_admin"), OP_UNPRINTABLE);
    assert_eq!(crate::sanitize_op(&"a".repeat(65)), OP_UNPRINTABLE);
    assert_eq!(crate::sanitize_op(""), OP_UNPRINTABLE);
    let e = StreamEntry::single(
        Scope::Deployment,
        Subject::Unverified,
        "</script><b>x",
        EventKind::denied(RecordedDecision::UnregisteredOp),
        Timestamp(1),
        cor(1),
    );
    assert_eq!(e.op, OP_UNPRINTABLE);
}

// ───────────────────────────── collapse ─────────────────────────────────

#[test]
fn a_window_collapses_by_space_principal_op_and_hashes_once() {
    let mut c = Collapser::new(60, 8);
    let key = |op: &str, who: &str| crate::CollapseKey {
        scope: Scope::Space(space()),
        about: None,
        subject: Subject::member(who),
        op: op.into(),
    };
    for n in 0..40u32 {
        c.observe(
            key("openTasks", "usr_1"),
            RecordedDecision::OpNotInRole,
            Timestamp(1_000 + u64::from(n)),
            cor(n),
        );
    }
    // A second decision for the same key joins the set, not a new entry.
    c.observe(
        key("openTasks", "usr_1"),
        RecordedDecision::PrincipalSuspended,
        Timestamp(1_050),
        cor(99),
    );
    // Different op, different key. Different principal, different key.
    c.observe(
        key("listOrders", "usr_1"),
        RecordedDecision::OpNotInRole,
        Timestamp(1_051),
        cor(100),
    );
    c.observe(
        key("openTasks", "usr_2"),
        RecordedDecision::OpNotInRole,
        Timestamp(1_052),
        cor(101),
    );
    assert_eq!(c.open_windows(), 3);
    assert!(
        c.close_due(Timestamp(1_030)).is_empty(),
        "window still open"
    );

    let mut out = c.close_due(Timestamp(1_100));
    out.sort_by_key(|e| (e.op.clone(), e.subject.render(), e.count));
    assert_eq!(out.len(), 3);
    let main = out
        .iter()
        .find(|e| e.op == "openTasks" && e.subject.render() == "user:usr_1")
        .expect("the collapsed window");
    assert_eq!(main.count, 41);
    assert_eq!(main.first_at, Timestamp(1_000));
    assert_eq!(main.last_at, Timestamp(1_050));
    assert_eq!(
        main.kind,
        EventKind::Denied(BTreeSet::from([
            RecordedDecision::OpNotInRole,
            RecordedDecision::PrincipalSuspended
        ]))
    );
    assert_eq!(
        main.correlations.len(),
        crate::event::CORRELATION_SAMPLE,
        "a bounded sample, not one per observation"
    );
    // The count is inside the hashed body — which is the whole reason
    // nothing is appended while the window is open.
    let a = hash_entry(ChainHash::GENESIS, main);
    let mut altered = main.clone();
    altered.count += 1;
    assert_ne!(a, hash_entry(ChainHash::GENESIS, &altered));
}

#[test]
fn the_cap_degrades_the_record_instead_of_growing_it() {
    let mut c = Collapser::new(60, 3);
    for n in 0..20u32 {
        c.observe(
            crate::CollapseKey {
                scope: Scope::Space(space()),
                about: None,
                subject: Subject::member(&format!("usr_{n}")),
                op: "openTasks".into(),
            },
            RecordedDecision::NoMembership,
            Timestamp(1_000),
            cor(n),
        );
    }
    let out = c.close_all();
    let suppressed: Vec<&StreamEntry> = out
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Suppressed { .. }))
        .collect();
    assert_eq!(out.len(), 4, "three entries plus one marker");
    assert_eq!(suppressed.len(), 1);
    assert_eq!(
        suppressed[0].kind,
        EventKind::Suppressed { further: 17 },
        "N further denials suppressed, as one entry"
    );
    assert_eq!(
        suppressed[0].subject.kind_str(),
        SubjectKind::Platform.as_str()
    );
}

#[test]
fn only_denials_collapse() {
    let recorder = Recorder::new(Collapser::new(60, 16));
    for n in 0..3u32 {
        recorder.observe(
            Observation {
                space: None,
                subject: Subject::platform("custody"),
                op: String::new(),
                kind: EventKind::KeyReleased {
                    purpose: "read".into(),
                },
                at: Timestamp(u64::from(n)),
                correlation: cor(n),
            },
            Relationship::None,
        );
    }
    // Three discrete acts, three entries, available immediately — a key
    // release is not a rate.
    let ready = recorder.drain_ready(Timestamp(0));
    assert_eq!(ready.len(), 3);
    assert!(ready.iter().all(|e| e.count == 1));
}

// ─────────────────── the decision, not the wire code ────────────────────

#[test]
fn every_deny_reason_maps_to_a_distinct_recorded_decision() {
    let reasons = [
        DenyReason::GrantSpaceMismatch,
        DenyReason::GrantExpired,
        DenyReason::ManifestChanged,
        DenyReason::OpNotInAllowlist,
        DenyReason::NoMembership,
        DenyReason::PrincipalSuspended,
        DenyReason::NoProfileForSpace,
        DenyReason::OpNotInRole { role: Role::Viewer },
        DenyReason::StepUpRequired,
        DenyReason::StepUpStale {
            age_secs: 1,
            max_age_secs: 0,
        },
        DenyReason::DelegationSpaceMismatch,
        DenyReason::DelegationExpired,
        DenyReason::DelegationBeyondDelegator,
        DenyReason::DelegationNotLive,
        DenyReason::OpNotDelegable,
    ];
    let mapped: BTreeSet<RecordedDecision> = reasons.iter().map(RecordedDecision::from).collect();
    assert_eq!(
        mapped.len(),
        reasons.len(),
        "no two door refusals may fold into one recorded decision"
    );
    // The distinctions the wire deliberately erases are separate here.
    assert_ne!(
        RecordedDecision::UnregisteredOp,
        RecordedDecision::OpNotInRole,
        "both answer 403 access_denied on the wire"
    );
    assert_ne!(
        RecordedDecision::GuardRejected,
        RecordedDecision::RowAbsent,
        "both answer not_found on the wire"
    );
    // Tags are stable and total.
    for d in [
        RecordedDecision::TokenUnverifiable,
        RecordedDecision::RowAbsent,
        RecordedDecision::Throttled,
        RecordedDecision::DeploymentTokenRefused,
        RecordedDecision::DeploymentTokenWrongIssuer,
        RecordedDecision::DeploymentTokenWrongAudience,
        RecordedDecision::DeploymentTokenExpired,
        RecordedDecision::OutsideDeploymentRealm,
        RecordedDecision::ConsoleKeyClosed,
    ] {
        assert_eq!(RecordedDecision::from_tag(d.tag()), Some(d));
    }
}

// ──────────────────────────── 7b · the alert ────────────────────────────

fn denial_series(space: &str, principal: &str, op: &str, n: u64) -> crate::CounterSeries {
    (
        DENIALS_METRIC.to_string(),
        vec![
            ("op".to_string(), op.to_string()),
            ("principal".to_string(), principal.to_string()),
            ("principal_kind".to_string(), "member".to_string()),
            ("space".to_string(), space.to_string()),
        ],
        n,
    )
}

#[test]
fn a_burst_from_one_principal_fires_and_a_normal_rate_does_not() {
    let rule = DenyVolumeRule::new(50, 300);
    let mut monitor = DenyVolumeMonitor::new(rule);

    // Baseline: both principals quiet.
    let firing = monitor.evaluate(
        &[
            denial_series("spc_alpha", "usr_burst", "openTasks", 0),
            denial_series("spc_alpha", "usr_normal", "openTasks", 0),
        ],
        Timestamp(1_000),
    );
    assert!(firing.is_empty());

    // usr_normal accumulates the ordinary trickle — a stale tab, a revoked
    // grant — over the whole window. usr_burst probes.
    let mut normal = 0u64;
    let mut burst = 0u64;
    let mut fired_at: Option<Timestamp> = None;
    for tick in 1..=10u64 {
        normal += 2;
        burst += 12;
        let now = Timestamp(1_000 + tick * 30);
        let firing = monitor.evaluate(
            &[
                denial_series("spc_alpha", "usr_burst", "openTasks", burst),
                denial_series("spc_alpha", "usr_normal", "openTasks", normal),
            ],
            now,
        );
        assert!(
            firing.iter().all(|f| f.principal == "usr_burst"),
            "a normal rate ({normal} in the window) must not fire"
        );
        if !firing.is_empty() && fired_at.is_none() {
            fired_at = Some(now);
            assert_eq!(firing.len(), 1);
            assert!(firing[0].count >= 50);
            assert_eq!(firing[0].space, "spc_alpha");
            assert!(firing[0].render().contains("usr_burst"));
        }
    }
    assert!(fired_at.is_some(), "the burst must fire");

    // And it clears by itself once the burst rolls out of the window.
    let quiet = monitor.evaluate(
        &[
            denial_series("spc_alpha", "usr_burst", "openTasks", burst),
            denial_series("spc_alpha", "usr_normal", "openTasks", normal),
        ],
        Timestamp(1_000 + 10 * 30 + 400),
    );
    assert!(quiet.is_empty(), "a firing is a level, not a latch");
}

#[test]
fn a_sweep_across_ops_is_one_firing_not_many_quiet_series() {
    // The pattern the alert exists for: one principal, one space, sixty
    // different ops, ten denials each. Per-op every series is tiny.
    let mut monitor = DenyVolumeMonitor::new(DenyVolumeRule::new(50, 300));
    let mut snapshot: Vec<crate::CounterSeries> = (0..60)
        .map(|i| denial_series("spc_alpha", "usr_sweep", &format!("op{i}"), 0))
        .collect();
    assert!(monitor.evaluate(&snapshot, Timestamp(0)).is_empty());
    for s in &mut snapshot {
        s.2 = 10;
    }
    let firing = monitor.evaluate(&snapshot, Timestamp(60));
    assert_eq!(firing.len(), 1);
    assert_eq!(firing[0].count, 600);
    assert_eq!(firing[0].principal, "usr_sweep");
}

#[test]
fn a_series_that_appears_already_high_fires_on_its_first_sample() {
    let mut monitor = DenyVolumeMonitor::new(DenyVolumeRule::new(50, 300));
    let firing = monitor.evaluate(
        &[denial_series("spc_alpha", "usr_new", "openTasks", 500)],
        Timestamp(10),
    );
    assert_eq!(
        firing.len(),
        1,
        "a counter that appears at 500 IS 500 denials"
    );
}

#[test]
fn an_unset_threshold_still_alerts_and_says_it_is_unconfigured() {
    let rule = DenyVolumeRule::default();
    assert!(!rule.configured);
    assert_eq!(rule.threshold, DenyVolumeRule::DEFAULT_THRESHOLD);
    let mut monitor = DenyVolumeMonitor::new(rule);
    let text = monitor.render(
        &[denial_series("spc_alpha", "usr_x", "openTasks", 999)],
        Timestamp(1),
    );
    assert!(text.contains("MISCONFIGURED"), "the dashboard says so");
    assert!(text.contains("FIRING deny-volume"));
    assert!(text.contains("No cross-replica aggregation"));
    let mut quiet = DenyVolumeMonitor::new(DenyVolumeRule::new(50, 300));
    assert!(quiet
        .render(&[], Timestamp(1))
        .contains("ok: nothing over threshold"));
}

#[test]
fn a_firing_is_traceable_to_the_requests_that_caused_it() {
    // This is what the correlation id buys: the counter says "usr_1 is
    // being denied a lot", and the stream says which requests.
    let recorder = Recorder::default();
    for n in 0..30u32 {
        recorder.deny(
            Some(&space()),
            Subject::member("usr_1"),
            "openTasks",
            RecordedDecision::OpNotInRole,
            &Stamp::new(Timestamp(1_000 + u64::from(n)), cor(n)),
            Relationship::Member,
        );
    }
    let mut store = InMemorySecurityStore::new();
    for e in recorder.drain_all(Timestamp(0)) {
        store.append(e).expect("appends");
    }
    let firing = crate::Firing {
        space: space().0.clone(),
        principal: "usr_1".into(),
        count: 30,
        threshold: 20,
        window_secs: 300,
        since: Timestamp(1_000),
        at: Timestamp(1_030),
    };
    let cors = correlations_for(&store, &firing, 64);
    assert_eq!(cors.len(), crate::event::CORRELATION_SAMPLE);
    assert!(cors.contains(&cor(0)));

    // A different principal's firing finds nothing in the same stream.
    let other = crate::Firing {
        principal: "usr_2".into(),
        ..firing
    };
    assert!(correlations_for(&store, &other, 64).is_empty());
}

// ───────────── D1a: pin the goldens before touching the encoder ─────────

/// **The backward-compatibility proof, and it is a number rather than an
/// argument**: the same technique `ordo-commandlog`'s `PRE_FLAG_GOLDEN`
/// uses (`crates/ordo-commandlog/src/tests.rs:637-690`) and D7 just landed
/// a second time on that chain. A golden derived from the code being
/// changed proves nothing about the code it was meant to check, so this
/// hash and this length are computed and written down **before**
/// `StreamEntry` gains `about` and before `encode_entry` gains a trailer,
/// against the encoder exactly as `chain.rs` reads today. If adding the
/// field moves a single byte of an entry that carries no target, this
/// constant stops matching and every stored security stream, every signed
/// checkpoint over it, and every golden that pins one is void.
///
/// A representative deployment entry: `Scope::Deployment`, an unverified
/// subject (no payload at all), a two-decision collapsed window, and two
/// sampled correlations, the shape a sweep's closed window actually takes.
const PRE_ABOUT_DEPLOYMENT_GOLDEN: &str =
    "50acd1abecfc111e8bea556e7ebd330ea0a888ec455a13b7597c203e1cea1202";
const PRE_ABOUT_DEPLOYMENT_LEN: usize = 120;

/// A representative space entry: `Scope::Space`, an attributed member
/// subject, a single decision, one correlation, the ordinary shape of a
/// member's own denial.
const PRE_ABOUT_SPACE_GOLDEN: &str =
    "2412191bd4b31c2f29fee6f7fc6d53cb7dba8b5f95765e6949fb98b854f04565";
const PRE_ABOUT_SPACE_LEN: usize = 133;

fn representative_deployment_entry() -> StreamEntry {
    StreamEntry {
        scope: Scope::Deployment,
        // No target: this is the representative shape from before `about`
        // existed, which is the one the golden below has to keep hashing
        // to the same place.
        about: None,
        subject: Subject::Unverified,
        op: "probe.sweep".into(),
        kind: EventKind::Denied(BTreeSet::from([
            RecordedDecision::TokenUnverifiable,
            RecordedDecision::UnregisteredOp,
        ])),
        first_at: Timestamp(1_000),
        last_at: Timestamp(1_090),
        count: 42,
        correlations: vec![cor(1), cor(2)],
    }
}

fn representative_space_entry() -> StreamEntry {
    StreamEntry {
        scope: Scope::Space(space()),
        about: None,
        subject: Subject::member("usr_1"),
        op: "openTasks".into(),
        kind: EventKind::denied(RecordedDecision::OpNotInRole),
        first_at: Timestamp(500),
        last_at: Timestamp(500),
        count: 1,
        correlations: vec![cor(9)],
    }
}

#[test]
fn a_deployment_entry_hashes_where_it_hashed() {
    let entry = representative_deployment_entry();
    let bytes = encode_entry(&entry);
    assert_eq!(
        bytes.len(),
        PRE_ABOUT_DEPLOYMENT_LEN,
        "a deployment entry with no target must render the bytes it \
         rendered before `about` existed, not one more"
    );
    assert_eq!(
        hash_entry(ChainHash::GENESIS, &entry).to_string(),
        PRE_ABOUT_DEPLOYMENT_GOLDEN,
        "adding `about` moved an existing deployment entry's hash; every \
         stored deployment stream is now unverifiable"
    );
}

#[test]
fn a_space_entry_hashes_where_it_hashed() {
    let entry = representative_space_entry();
    let bytes = encode_entry(&entry);
    assert_eq!(
        bytes.len(),
        PRE_ABOUT_SPACE_LEN,
        "a space entry must never move: it cannot carry a target at all"
    );
    assert_eq!(
        hash_entry(ChainHash::GENESIS, &entry).to_string(),
        PRE_ABOUT_SPACE_GOLDEN,
        "adding `about` moved an existing space entry's hash; every stored \
         space stream is now unverifiable"
    );
}

// ───── A deployment token's kind, client and sid (subject tag 2) ─────

fn token_entry(sid: &str) -> StreamEntry {
    StreamEntry {
        scope: Scope::Deployment,
        about: Some(SpaceId("system".into())),
        subject: Subject::deployment(
            "p_ada",
            TokenFacts {
                kind: "person".into(),
                client: "console".into(),
                sid: sid.into(),
            },
        ),
        op: "std.deployment.apply".into(),
        kind: EventKind::Authenticated,
        first_at: Timestamp(2_000),
        last_at: Timestamp(2_000),
        count: 1,
        correlations: vec![cor(3)],
    }
}

/// Link `entries` from genesis, as an append does.
fn link(entries: Vec<StreamEntry>) -> Vec<ChainedEntry> {
    let mut prev = ChainHash::GENESIS;
    entries
        .into_iter()
        .map(|entry| {
            let hash = hash_entry(prev, &entry);
            let e = ChainedEntry { entry, prev, hash };
            prev = hash;
            e
        })
        .collect()
}

#[test]
fn a_deployment_tokens_facts_are_in_the_hashed_body_and_decode_back() {
    let entry = token_entry("s_1");
    let bytes = encode_entry(&entry);
    // DOMAIN, then scope 0x00 (deployment), then subject tag 2.
    assert_eq!(bytes[DOMAIN.len()], 0x00);
    assert_eq!(bytes[DOMAIN.len() + 1], 0x02);
    let back = decode_entry(&bytes).expect("decodes");
    assert_eq!(back, entry);
    let facts = back.subject.token().expect("facts");
    assert_eq!(
        (
            facts.kind.as_str(),
            facts.client.as_str(),
            facts.sid.as_str()
        ),
        ("person", "console", "s_1")
    );
    // Each fact moves the hash.
    let base = hash_entry(ChainHash::GENESIS, &entry);
    for changed in [
        TokenFacts {
            kind: "machine".into(),
            client: "console".into(),
            sid: "s_1".into(),
        },
        TokenFacts {
            kind: "person".into(),
            client: "cli".into(),
            sid: "s_1".into(),
        },
        TokenFacts {
            kind: "person".into(),
            client: "console".into(),
            sid: "s_2".into(),
        },
    ] {
        let mut other = entry.clone();
        other.subject = Subject::deployment("p_ada", changed);
        assert_ne!(hash_entry(ChainHash::GENESIS, &other), base);
    }
    // Without facts, the same member is the old tag 1 and a different body.
    let mut plain = entry.clone();
    plain.subject = Subject::member("p_ada");
    assert_eq!(encode_entry(&plain)[DOMAIN.len() + 1], 0x01);
    assert_ne!(plain.subject, entry.subject);
}

#[test]
fn an_old_format_chain_still_verifies_and_new_entries_extend_it() {
    // Two entries in the shapes written before tag 2 existed, still
    // hashing where the goldens pin them.
    let mut chain = link(vec![
        representative_deployment_entry(),
        representative_space_entry(),
    ]);
    assert_eq!(chain[0].hash.to_string(), PRE_ABOUT_DEPLOYMENT_GOLDEN);
    verify_stream(&Scope::Deployment, &chain).expect("old chain verifies");

    // An old member subject spelled byte for byte from the frozen layout,
    // not by the encoder: tag 1, the attribution, the kind. It decodes
    // with no token facts.
    let entry = representative_space_entry();
    let mut old = Vec::new();
    old.extend_from_slice(DOMAIN);
    old.push(0x01);
    old.extend_from_slice(&(space().0.len() as u64).to_be_bytes());
    old.extend_from_slice(space().0.as_bytes());
    old.push(0x01);
    let who = "user:usr_1";
    old.extend_from_slice(&(who.len() as u64).to_be_bytes());
    old.extend_from_slice(who.as_bytes());
    old.push(0x00);
    let rest = &encode_entry(&entry)[old.len()..];
    old.extend_from_slice(rest);
    assert_eq!(old, encode_entry(&entry));
    assert_eq!(decode_entry(&old).expect("decodes").subject.token(), None);

    // A new entry chains on after them, and the whole chain verifies.
    let prev = chain[1].hash;
    let next = token_entry("s_1");
    let hash = hash_entry(prev, &next);
    chain.push(ChainedEntry {
        entry: next,
        prev,
        hash,
    });
    verify_stream(&Scope::Deployment, &chain).expect("mixed chain verifies");

    // Changing the sid on the stored entry breaks the chain there.
    chain[2].entry = token_entry("s_forged");
    let err = verify_stream(&Scope::Deployment, &chain).expect_err("tampered");
    assert_eq!(err.index, 2);
    assert_eq!(err.kind, TamperKind::HashMismatch);
}

// ───────── D1b: the sweep, on the record before and after the fix ───────

/// **The defect §0.3 names, shown failing before the fix.**
///
/// A stranger denied at a hundred different spaces, same principal, same
/// op. §6.1 pins the collapse key at `(space, principal, op)` precisely so
/// this produces a hundred keys; `collapse.rs` believed `scope` stood in
/// for space, which is true for `Scope::Space` and false for
/// `Scope::Deployment`: every one of these observations carries the same
/// scope, so before the fix they collapse into **one** entry with
/// `count: 100`, indistinguishable from a hundred retries against one
/// space. See this test's assertion message for the exact pre-fix shape.
#[test]
fn a_sweep_over_a_hundred_spaces_is_a_hundred_entries() {
    let recorder = Recorder::default();
    for n in 0..100u32 {
        recorder.deny(
            Some(&SpaceId(format!("spc_target_{n:03}"))),
            Subject::Unverified,
            "openTasks",
            RecordedDecision::TokenUnverifiable,
            &Stamp::new(Timestamp(1_000), cor(n)),
            Relationship::None,
        );
    }
    let entries = recorder.drain_all(Timestamp(2_000));
    let deployment: Vec<&StreamEntry> = entries
        .iter()
        .filter(|e| e.scope == Scope::Deployment)
        .collect();
    assert_eq!(
        deployment.len(),
        100,
        "a hundred distinct targets must produce a hundred entries, not \
         one collapsed window: got {} entr{} with count(s) {:?}",
        deployment.len(),
        if deployment.len() == 1 { "y" } else { "ies" },
        deployment.iter().map(|e| e.count).collect::<Vec<_>>()
    );
}

/// §1.3's trap, checked directly against the cap rather than trusted from
/// reading the code: the per-window cap counts distinct keys **per scope**,
/// and adding `about` to the key must not make it count per target.
///
/// If it did, a sweep across caller-chosen space ids would buy each target
/// its own budget and the attacker would size the record, the exact defect
/// the cap exists to prevent, one level up.
#[test]
fn the_window_cap_counts_per_scope_and_not_per_target() {
    let mut c = Collapser::new(60, 3);
    for n in 0..20u32 {
        c.observe(
            crate::CollapseKey {
                scope: Scope::Deployment,
                // Twenty distinct targets, one subject, one op: the shape
                // of a sweep, not of a repeat.
                about: Some(SpaceId(format!("spc_target_{n:03}"))),
                subject: Subject::Unverified,
                op: "openTasks".into(),
            },
            RecordedDecision::TokenUnverifiable,
            Timestamp(1_000),
            cor(n),
        );
    }
    let out = c.close_all();
    let suppressed: Vec<&StreamEntry> = out
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Suppressed { .. }))
        .collect();
    assert_eq!(
        out.len(),
        4,
        "the cap is 3, so three distinct-target entries plus one marker, \
         not twenty entries, which is what a per-target cap would allow"
    );
    assert_eq!(suppressed.len(), 1);
    assert_eq!(
        suppressed[0].kind,
        EventKind::Suppressed { further: 17 },
        "seventeen distinct targets past the cap, collapsed into one marker"
    );
    assert_eq!(
        suppressed[0].about, None,
        "the marker aggregates many targets and must not pick one to name"
    );
}

/// §1.2's admission rule for the space, checked the same way `sanitize_op`
/// already is: all-or-nothing, never a partial strip.
#[test]
fn a_hostile_space_id_becomes_the_sentinel() {
    assert_eq!(crate::sanitize_space("spc_alpha"), "spc_alpha");
    assert_eq!(crate::sanitize_space("spc-alpha_9"), "spc-alpha_9");
    // Newlines and quotes are how a record gets forged by the person it is
    // about.
    assert_eq!(
        crate::sanitize_space("spc_x\ndeny usr_admin"),
        crate::event::SPACE_UNPRINTABLE
    );
    assert_eq!(
        crate::sanitize_space(&"s".repeat(200)),
        crate::event::SPACE_UNPRINTABLE
    );
    assert_eq!(crate::sanitize_space(""), crate::event::SPACE_UNPRINTABLE);
    // Not `sanitize_op`'s alphabet: a `.` is off-shape for a space id too.
    assert_eq!(
        crate::sanitize_space("spc.alpha"),
        crate::event::SPACE_UNPRINTABLE
    );

    // And through the recorder: a stranger names a hostile string as the
    // space, and the deployment entry's `about` carries the sentinel, never
    // the caller's bytes.
    let recorder = Recorder::default();
    recorder.observe(
        Observation {
            space: Some(SpaceId("</script><b>x".into())),
            subject: Subject::Unverified,
            op: "openTasks".into(),
            kind: EventKind::denied(RecordedDecision::TokenUnverifiable),
            at: Timestamp(1),
            correlation: cor(1),
        },
        Relationship::None,
    );
    let entries = recorder.drain_all(Timestamp(0));
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].about,
        Some(SpaceId(crate::event::SPACE_UNPRINTABLE.into()))
    );
}

/// §1.2's invariant, checked at the one place it can be violated: routing.
/// However the caller names a space, an entry that lands in its **own**
/// stream must never also carry that space as `about`: the scope already
/// is the space, and a second field spelling the same fact could disagree
/// with the first.
#[test]
fn a_space_scope_entry_carries_no_target() {
    let (engine, member) = engine_with_member();
    let recorder = Recorder::default();
    recorder.observe(
        Observation {
            space: Some(space()),
            subject: Subject::member(&member.principal.0),
            op: "openTasks".into(),
            kind: EventKind::Authenticated,
            at: Timestamp(1),
            correlation: cor(1),
        },
        relationship(&engine, &member, &space()),
    );
    let entries = recorder.drain_all(Timestamp(0));
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].scope, Scope::Space(space()));
    assert_eq!(
        entries[0].about, None,
        "a space-scope entry already IS the space; it must not also name \
         one in `about`"
    );

    // The same check on the encoded bytes, not only on the field: a
    // space-scope entry with `about` set would still decode, so the
    // invariant has to be a property of every producer rather than of the
    // wire. This asserts the debug-time guard fires on the violation.
    let mut violating = entries[0].clone();
    violating.about = Some(space());
    let result = std::panic::catch_unwind(|| encode_entry(&violating));
    assert!(
        result.is_err(),
        "encode_entry must refuse (in a debug build) a space-scope entry \
         that carries a target"
    );
}

/// §1.2's trailer, refusing an unknown tag exactly the way
/// `ordo_commandlog::chain`'s does: a byte that is not a known tag is
/// trailing bytes, whatever position it is in.
#[test]
fn a_trailer_with_an_unknown_tag_refuses() {
    let entry = StreamEntry::single(
        Scope::Deployment,
        Subject::Unverified,
        "openTasks",
        EventKind::denied(RecordedDecision::TokenUnverifiable),
        Timestamp(1),
        cor(1),
    );
    let mut bytes = encode_entry(&entry);
    // No trailer today past 0x01: 0x02 is a known-shape byte for a second
    // tag that does not exist yet, and 0xFF is unknown by any reading.
    for unknown in [0x02u8, 0xFFu8] {
        let mut hostile = bytes.clone();
        hostile.push(unknown);
        assert_eq!(
            decode_entry(&hostile),
            Err(DecodeError::TrailingBytes),
            "tag {unknown:#04x} names no trailer and must refuse rather \
             than decode as silence"
        );
    }
    // The control: the entry with no trailer at all still decodes.
    assert!(decode_entry(&bytes).is_ok());
    bytes.push(TRAILER_ABOUT_TEST_ONLY);
    bytes.extend_from_slice(&8u64.to_be_bytes());
    bytes.extend_from_slice(b"spc_late");
    assert!(
        decode_entry(&bytes).is_ok(),
        "the one known tag, well-formed, still decodes"
    );
}

/// A trailer's tag, mirrored here rather than exported, so this test file
/// does not need `chain::TRAILER_ABOUT` to be `pub(crate)` beyond what the
/// module already exposes for its own decoder.
const TRAILER_ABOUT_TEST_ONLY: u8 = 0x01;

/// §1.2's canonicity rule: silence is the one spelling of "no target", so
/// an explicit `0x00` (which names no tag) is refused rather than read as
/// a verbose no-op. Two renderings of one entry would let one entry be
/// re-hashed two ways.
#[test]
fn an_explicit_empty_trailer_is_trailing_bytes() {
    let entry = StreamEntry::single(
        Scope::Deployment,
        Subject::Unverified,
        "openTasks",
        EventKind::denied(RecordedDecision::TokenUnverifiable),
        Timestamp(1),
        cor(1),
    );
    let mut bytes = encode_entry(&entry);
    bytes.push(0x00);
    assert_eq!(decode_entry(&bytes), Err(DecodeError::TrailingBytes));
}

#[test]
fn a_minted_correlation_is_typed_unique_and_carries_nothing() {
    let a = correlation::mint();
    let b = correlation::mint();
    assert_ne!(a, b);
    assert!(a.0.starts_with("cor_"));
    assert_eq!(a.0.len(), 4 + 16);
    // Adopting an outside id keeps a well-formed one and refuses to write a
    // hostile one into a hashed record.
    assert_eq!(correlation::adopt("cg_00ff").0, "cg_00ff");
    assert!(correlation::adopt("bad id\nDENY").0.starts_with("cor_"));
}

// ─────────────────── custody acts, owner-ruled 2026-09-14 ─────────────────

#[test]
fn a_custody_act_about_a_space_is_recorded_in_that_space() {
    assert_eq!(
        route(
            Some(&space()),
            &Subject::platform("keyprovider"),
            Relationship::Custodian
        ),
        Scope::Space(space()),
        "the tenant is the party who should see their key being opened"
    );
}

#[test]
fn a_custody_act_about_no_space_stays_the_deployments() {
    assert_eq!(
        route(
            None,
            &Subject::platform("keyprovider"),
            Relationship::Custodian
        ),
        Scope::Deployment
    );
}

#[test]
fn an_unverified_subject_never_reaches_a_tenant_stream_even_as_custodian() {
    assert_eq!(
        route(
            Some(&space()),
            &Subject::Unverified,
            Relationship::Custodian
        ),
        Scope::Deployment,
        "nothing was proven, so no relationship, custody included, can exist"
    );
}
