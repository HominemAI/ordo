//! Layer-1 pinning against a real chain, checklist 8q.
//!
//! `deployment.rs` proves the pin reaches a caller and verifies. This file
//! proves the pin FOLLOWS THE CHAIN, against groups appended directly, so
//! that the property is pinned independently of which door wrote them. (It
//! was written when the host built groups in memory only and every response
//! pinned genesis; since checklist 11-28 the host's own writes reach the
//! chain too.)
//!
//! Monotonicity is the property that makes a pin evidence rather than
//! decoration: a client holding one can say a deployment rewrote history
//! only because the position it was handed can never go backwards. So it is
//! asserted here, where groups actually reach the chain.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_api::attest::AttestationSource;
use ordo_commandlog::sign::Ed25519Signer;
use ordo_commandlog::{
    Actor, ChainHash, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Image, SpaceId,
    Timestamp, Value,
};
use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::attest::PgAttestation;

const SPACE: &str = "attest_space";

async fn pool(db: &str) -> Pool {
    let _ = Pg::named(db);
    Pool::connect(PoolConfig {
        db: db.into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot")
}

fn attestation(pool: Pool) -> PgAttestation {
    PgAttestation::new(
        pool,
        Box::new(Ed25519Signer::from_seed("attest-test", &[9u8; 32])),
    )
}

fn group(n: usize) -> CommandGroup {
    let after: Image = [
        ("id".to_string(), Value::Id(format!("txn_{n}"))),
        ("amount".to_string(), Value::Int(n as i64 * 100)),
    ]
    .into_iter()
    .collect();
    CommandGroup {
        correlation: CorrelationId(format!("pin_{n}")),
        actor: Actor::User(format!("usr_{n}")),
        commands: vec![Command {
            id: CommandId(format!("cmd_{n}")),
            space: SpaceId(SPACE.to_string()),
            entity: "transactions".into(),
            row_id: format!("txn_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(after),
            at: Timestamp(1_000 + n as u64),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

/// The three properties a client depends on, in one run because they are one
/// story: an empty chain still pins, an append moves the pin, and the pin
/// never goes backwards.
#[tokio::test(flavor = "multi_thread")]
async fn the_pin_follows_the_chain_and_never_goes_backwards() {
    let db = "ordo_attest_chain";
    let p = pool(db).await;
    let _ = p.rows("DROP TABLE IF EXISTS \"$commandlog\"").await;
    let tx = p.begin().await.unwrap();
    drop(tx); // forces the platform tables into existence via boot
    let p = pool(db).await;
    let _ = p
        .rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = '{SPACE}'"
        ))
        .await;

    let at = attestation(p.clone());

    // An empty chain pins GENESIS rather than nothing. "Always on" that
    // switches on after the first write is not always on, and a client whose
    // first request is a read must hold something — otherwise it cannot tell
    // "no history yet" from "this deployment does not attest".
    let first = at.head(SPACE).await.expect("an empty chain still pins");
    assert_eq!(first.upto, 0);
    assert_eq!(first.head, "0".repeat(64), "genesis is the all-zero head");
    assert_eq!(first.key_id, "attest-test");

    // Append, and the pin moves.
    let mut last = first.clone();
    for n in 0..3 {
        let tx = p.begin().await.unwrap();
        tx.append_group(&group(n)).await.expect("append");
        tx.commit().await.expect("commit");

        let now = at.head(SPACE).await.expect("pin");
        assert_eq!(
            now.upto,
            last.upto + 1,
            "one group moves the position by exactly one"
        );
        assert!(
            now.upto > last.upto && now.head != last.head,
            "an append must change both the position and the head"
        );
        assert_ne!(
            now.signature, last.signature,
            "a new head must be signed afresh, or the pin would attest the \
             wrong history"
        );
        last = now;
    }

    // Re-reading without appending yields the identical pin, which is what
    // makes the caching sound AND what a client needs: a pin that changed on
    // its own would be noise, and noise gets ignored.
    let again = at.head(SPACE).await.expect("pin");
    assert_eq!(again, last, "an unchanged chain pins identically");
}

/// The signature is over the log's own canonical bytes, so a client verifies
/// a pin with the same routine that checks a stored checkpoint. Publishing a
/// second attestation format for one head would let the two disagree.
#[tokio::test(flavor = "multi_thread")]
async fn a_pin_verifies_with_the_checkpoint_routine() {
    let db = "ordo_attest_verify";
    let p = pool(db).await;
    let tx = p.begin().await.unwrap();
    drop(tx);
    let p = pool(db).await;
    let _ = p
        .rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = '{SPACE}'"
        ))
        .await;
    let tx = p.begin().await.unwrap();
    tx.append_group(&group(41)).await.expect("append");
    tx.commit().await.expect("commit");

    let pin = attestation(p).head(SPACE).await.expect("pin");
    let signer = Ed25519Signer::from_seed("attest-test", &[9u8; 32]);
    let bytes = ordo_commandlog::Checkpoint::signed_bytes(
        &SpaceId(SPACE.to_string()),
        pin.upto,
        unhex_hash(&pin.head),
        Timestamp(0),
    );
    assert!(
        ordo_commandlog::store::Signer::verify(&signer, &bytes, &unhex(&pin.signature)),
        "a pin a client cannot check is decoration"
    );

    // …and the same signer refuses a pin whose position was tampered with,
    // which is the whole point of signing the position rather than the head
    // alone.
    let lying = ordo_commandlog::Checkpoint::signed_bytes(
        &SpaceId(SPACE.to_string()),
        pin.upto + 1,
        unhex_hash(&pin.head),
        Timestamp(0),
    );
    assert!(
        !ordo_commandlog::store::Signer::verify(&signer, &lying, &unhex(&pin.signature)),
        "moving the position must break the signature"
    );
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn unhex_hash(s: &str) -> ChainHash {
    let mut out = [0u8; 32];
    out.copy_from_slice(&unhex(s));
    ChainHash(out)
}

/// **`head_at` signs the head the rows were read at** (C1 of
/// `plan-client-cache.md`). At the current position it is the ordinary pin;
/// at an earlier one it is the hash of the group at that position, signed over
/// the log's own checkpoint bytes, and not the latest head.
#[tokio::test(flavor = "multi_thread")]
async fn head_at_signs_the_head_the_rows_were_read_at() {
    use ordo_commandlog::store::Signer as _;
    let db = "ordo_attest_head_at";
    let p = pool(db).await;
    let _ = p
        .rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = '{SPACE}'"
        ))
        .await;
    let at = attestation(p.clone());
    for n in 0..3 {
        let tx = p.begin().await.unwrap();
        tx.append_group(&group(n)).await.expect("append");
        tx.commit().await.expect("commit");
    }

    let latest = at.head(SPACE).await.expect("pin");
    assert_eq!(latest.upto, 3);
    let same = at.head_at(SPACE, 3).await.expect("a pin at the head");
    assert_eq!(
        (same.upto, &same.head, &same.signature),
        (latest.upto, &latest.head, &latest.signature),
        "at the current position, head_at is the ordinary pin"
    );

    let earlier = at
        .head_at(SPACE, 2)
        .await
        .expect("a pin at an earlier position");
    assert_eq!(earlier.upto, 2);
    let hash = p
        .rows(&format!(
            "SELECT encode(\"hash\", 'hex') FROM \"$commandlog\" \
             WHERE \"space\" = '{SPACE}' AND \"idx\" = 1"
        ))
        .await
        .unwrap()[0][0]
        .clone()
        .unwrap();
    assert_eq!(
        earlier.head, hash,
        "the head at position 2 is the hash of the group at index 1"
    );
    assert_ne!(earlier.head, latest.head);

    let signer = Ed25519Signer::from_seed("attest-test", &[9u8; 32]);
    let mut head = [0u8; 32];
    for (i, b) in head.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).unwrap();
    }
    let bytes = ordo_commandlog::Checkpoint::signed_bytes(
        &SpaceId(SPACE.to_string()),
        2,
        ChainHash(head),
        Timestamp(0),
    );
    let expected: String = signer
        .sign(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        earlier.signature, expected,
        "signed over the checkpoint bytes at that position"
    );

    assert!(
        at.head_at(SPACE, 4).await.is_none(),
        "a position the chain has not reached is not pinned"
    );
}
