//! **Enrolment E2: tickets.** Minted, single-use, expiring, redeemable once.
//!
//! The memory tests prove the rules. The Postgres tests prove the two things
//! a memory store cannot: that single use holds between two replicas, and
//! that the table holds no ticket.
//!
//! Databases are prefixed `ordo_tickets_`. Needs the conformance cluster
//! (`scripts/conformance-pg.sh start`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ordo_host::tickets::{
    redeem, MemoryTicketStore, PgTicketStore, Purpose, TicketError, TicketSpec, TicketStore,
    Tickets, Want, DEFAULT_TTL_SECS, MAX_TTL_SECS,
};
use ordo_identity::principal::PrincipalId;

const T0: u64 = 1_000_000;

fn spec(purpose: Purpose) -> TicketSpec {
    TicketSpec {
        purpose,
        principal: Some(PrincipalId("usr_ana".into())),
        display: "Ana".into(),
        space: None,
        role: None,
        issued_by: "usr_admin".into(),
        ttl_secs: 0,
    }
}

/// A `Tickets` whose clock the test moves.
fn tickets(store: Arc<dyn TicketStore>) -> (Tickets, Arc<AtomicU64>) {
    let clock = Arc::new(AtomicU64::new(T0));
    let c = clock.clone();
    (
        Tickets::new(store).with_clock(Box::new(move || c.load(Ordering::SeqCst))),
        clock,
    )
}

/// **A redeemed ticket cannot be redeemed again.**
#[test]
fn a_redeemed_ticket_cannot_be_redeemed_again() {
    let (t, _) = tickets(Arc::new(MemoryTicketStore::new()));
    let issued = t.issue(spec(Purpose::Password)).expect("issued");

    let first = t.redeem(&issued.ticket).expect("the first redemption");
    assert_eq!(first.purpose, Purpose::Password);
    assert_eq!(first.principal, Some(PrincipalId("usr_ana".into())));
    assert_eq!(first.display, "Ana");

    assert_eq!(
        t.redeem(&issued.ticket),
        Err(TicketError::Refused),
        "the second redemption of one ticket must be refused"
    );
}

/// **An expired ticket is refused with the door's one answer.** Unknown,
/// expired, redeemed and malformed are the same value and the same words.
#[test]
fn an_expired_ticket_is_refused_with_the_one_answer() {
    let (t, clock) = tickets(Arc::new(MemoryTicketStore::new()));
    let mut short = spec(Purpose::Passkey);
    short.ttl_secs = 60;
    let expiring = t.issue(short).expect("issued");
    let redeemed = t.issue(spec(Purpose::Passkey)).expect("issued");
    t.redeem(&redeemed.ticket).expect("redeemed once");

    // One second before it ends it still works; at its end it does not.
    clock.store(T0 + 60, Ordering::SeqCst);
    let expired = t.redeem(&expiring.ticket).expect_err("expired");

    let unknown = t
        .redeem(&format!("tkt_{}", "ab".repeat(32)))
        .expect_err("unknown");
    let again = t.redeem(&redeemed.ticket).expect_err("redeemed");
    let malformed = t.redeem("not a ticket").expect_err("malformed");
    let empty = t.redeem("").expect_err("empty");

    for other in [&unknown, &again, &malformed, &empty] {
        assert_eq!(&expired, other);
        assert_eq!(expired.to_string(), other.to_string());
    }
    assert_eq!(expired, TicketError::Refused);
}

#[test]
fn a_ticket_is_good_until_the_second_before_it_ends() {
    let (t, clock) = tickets(Arc::new(MemoryTicketStore::new()));
    let mut s = spec(Purpose::Password);
    s.ttl_secs = 60;
    let issued = t.issue(s).expect("issued");
    assert_eq!(issued.expires_at, T0 + 60);
    clock.store(T0 + 59, Ordering::SeqCst);
    assert!(t.redeem(&issued.ticket).is_ok());
}

#[test]
fn a_ticket_never_lives_longer_than_the_maximum_and_defaults_to_three_days() {
    let (t, _) = tickets(Arc::new(MemoryTicketStore::new()));
    let mut forever = spec(Purpose::Password);
    forever.ttl_secs = u64::MAX / 2;
    assert_eq!(t.issue(forever).unwrap().expires_at, T0 + MAX_TTL_SECS);
    assert_eq!(
        t.issue(spec(Purpose::Password)).unwrap().expires_at,
        T0 + DEFAULT_TTL_SECS
    );
}

/// An invitation carries the tenant and the role it admits at, and a ticket
/// for a stranger carries no principal.
#[test]
fn a_ticket_carries_what_the_issuer_named() {
    let (t, _) = tickets(Arc::new(MemoryTicketStore::new()));
    let mut s = spec(Purpose::Invitation);
    s.principal = None;
    s.space = Some("spc_a".into());
    s.role = Some("editor".into());
    let issued = t.issue(s).unwrap();
    let got = t.redeem(&issued.ticket).unwrap();
    assert_eq!(got.principal, None);
    assert_eq!(got.purpose, Purpose::Invitation);
    assert_eq!(got.space.as_deref(), Some("spc_a"));
    assert_eq!(got.role.as_deref(), Some("editor"));
}

/// A ticket presented by the wrong person, for the wrong purpose or at the
/// wrong tenant is refused and **not spent**, so it is still there for the
/// person it was for. Otherwise anybody who saw a ticket could burn it.
#[test]
fn a_ticket_presented_by_the_wrong_person_is_refused_and_not_spent() {
    let (t, _) = tickets(Arc::new(MemoryTicketStore::new()));
    let mut s = spec(Purpose::Invitation);
    s.space = Some("spc_a".into());
    let issued = t.issue(s).unwrap();
    let right = Want {
        purpose: Some(Purpose::Invitation),
        space: Some("spc_a"),
        holder: Some("usr_ana"),
    };
    for wrong in [
        Want {
            holder: Some("usr_thief"),
            ..right.clone()
        },
        Want {
            space: Some("spc_b"),
            ..right.clone()
        },
        Want {
            purpose: Some(Purpose::Password),
            ..right.clone()
        },
    ] {
        assert_eq!(
            t.redeem_for(&issued.ticket, &wrong),
            Err(TicketError::Refused)
        );
    }
    assert!(
        t.redeem_for(&issued.ticket, &right).is_ok(),
        "still there for Ana"
    );
    assert_eq!(
        t.redeem_for(&issued.ticket, &right),
        Err(TicketError::Refused)
    );
}

/// Two tickets are two secrets, and one ticket is not another's hash.
#[test]
fn tickets_are_distinct_and_unguessable_in_shape() {
    let (t, _) = tickets(Arc::new(MemoryTicketStore::new()));
    let a = t.issue(spec(Purpose::Password)).unwrap().ticket;
    let b = t.issue(spec(Purpose::Password)).unwrap().ticket;
    assert_ne!(a, b);
    assert!(a.starts_with("tkt_"));
    assert_eq!(a.len(), 4 + 64);
    assert!(redeem(&MemoryTicketStore::new(), &a, T0).is_err());
}

/// The sweep removes what has ended and counts what was never used.
#[test]
fn the_sweep_counts_tickets_that_expired_unused() {
    let (t, clock) = tickets(Arc::new(MemoryTicketStore::new()));
    let used = t.issue(spec(Purpose::Password)).unwrap();
    t.redeem(&used.ticket).unwrap();
    t.issue(spec(Purpose::Password)).unwrap();
    t.issue(spec(Purpose::Password)).unwrap();
    assert_eq!(t.sweep().unwrap(), 0, "nothing has ended yet");
    clock.store(T0 + DEFAULT_TTL_SECS, Ordering::SeqCst);
    assert_eq!(t.sweep().unwrap(), 2, "two expired unused, one was used");
    assert_eq!(t.sweep().unwrap(), 0, "and they are gone");
}

// ── Postgres ─────────────────────────────────────────────────────────────

fn pool_config(db: &str) -> ordo_exec::PoolConfig {
    ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 1,
        ..ordo_exec::PoolConfig::from_env()
    }
}

async fn prepared(db: &str) -> ordo_exec::Pool {
    let pg = ordo_conformance::Pg::named(db);
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(pool_config(db))
        .await
        .expect("pool");
    ordo_host::tickets::ensure_table(&pool)
        .await
        .expect("ticket table");
    let _ = pool.rows("DELETE FROM \"$ticket\"").await;
    pool
}

/// **Single use holds between two replicas.** Two stores over one database
/// are two processes. Sixteen threads race one ticket across them and
/// exactly one wins.
#[tokio::test(flavor = "multi_thread")]
async fn two_replicas_racing_one_ticket_cannot_both_redeem() {
    let db = "ordo_tickets_race";
    let _pool = prepared(db).await;
    let a: Arc<dyn TicketStore> = Arc::new(PgTicketStore::open(pool_config(db)).expect("a"));
    let b: Arc<dyn TicketStore> = Arc::new(PgTicketStore::open(pool_config(db)).expect("b"));
    let (issuer, _) = tickets(a.clone());
    let ticket = issuer.issue(spec(Purpose::Password)).unwrap().ticket;

    let mut handles = Vec::new();
    for i in 0..16 {
        let store = if i % 2 == 0 { a.clone() } else { b.clone() };
        let ticket = ticket.clone();
        handles.push(std::thread::spawn(move || {
            redeem(store.as_ref(), &ticket, T0).is_ok()
        }));
    }
    let wins = handles
        .into_iter()
        .map(|h| h.join().expect("thread"))
        .filter(|w| *w)
        .count();
    assert_eq!(wins, 1, "one ticket, one redemption, across two replicas");
}

/// A ticket redeemed through a store that has since been dropped is still
/// redeemed when a new one opens, and the table never held the ticket.
#[tokio::test(flavor = "multi_thread")]
async fn the_table_holds_a_hash_and_a_redemption_survives_a_restart() {
    let db = "ordo_tickets_restart";
    let pool = prepared(db).await;

    let ticket = {
        let store: Arc<dyn TicketStore> =
            Arc::new(PgTicketStore::open(pool_config(db)).expect("first"));
        let (t, _) = tickets(store);
        let issued = t.issue(spec(Purpose::Passkey)).unwrap();
        t.redeem(&issued.ticket).expect("redeemed");
        issued.ticket
    };

    let store: Arc<dyn TicketStore> =
        Arc::new(PgTicketStore::open(pool_config(db)).expect("second"));
    assert_eq!(
        redeem(store.as_ref(), &ticket, T0),
        Err(TicketError::Refused),
        "the redemption did not survive the restart"
    );

    let rows = pool
        .rows("SELECT \"hash\", \"redeemed_at\" FROM \"$ticket\"")
        .await
        .expect("read");
    assert_eq!(rows.len(), 1);
    let stored = rows[0][0].clone().unwrap();
    assert_ne!(stored, ticket, "the table holds the ticket itself");
    assert!(!stored.contains(&ticket[4..]));
    assert!(rows[0][1].is_some(), "and it says when it was redeemed");
}

/// The same refusal-without-spending, through Postgres.
#[tokio::test(flavor = "multi_thread")]
async fn the_pg_store_does_not_spend_a_ticket_for_the_wrong_holder() {
    let db = "ordo_tickets_want";
    let _pool = prepared(db).await;
    let store: Arc<dyn TicketStore> = Arc::new(PgTicketStore::open(pool_config(db)).expect("s"));
    let (t, _) = tickets(store);
    let mut s = spec(Purpose::Invitation);
    s.space = Some("spc_a".into());
    let issued = t.issue(s).unwrap();
    let wrong = Want {
        holder: Some("usr_thief"),
        ..Default::default()
    };
    assert_eq!(
        t.redeem_for(&issued.ticket, &wrong),
        Err(TicketError::Refused)
    );
    let wrong = Want {
        space: Some("spc_b"),
        ..Default::default()
    };
    assert_eq!(
        t.redeem_for(&issued.ticket, &wrong),
        Err(TicketError::Refused)
    );
    let right = Want {
        purpose: Some(Purpose::Invitation),
        space: Some("spc_a"),
        holder: Some("usr_ana"),
    };
    assert!(t.redeem_for(&issued.ticket, &right).is_ok());
}

/// The sweep, through Postgres, counts unused expiries.
#[tokio::test(flavor = "multi_thread")]
async fn the_pg_sweep_removes_ended_tickets_and_counts_the_unused() {
    let db = "ordo_tickets_sweep";
    let _pool = prepared(db).await;
    let store: Arc<dyn TicketStore> = Arc::new(PgTicketStore::open(pool_config(db)).expect("s"));
    let (t, clock) = tickets(store);
    let used = t.issue(spec(Purpose::Password)).unwrap();
    t.redeem(&used.ticket).unwrap();
    t.issue(spec(Purpose::Password)).unwrap();
    clock.store(T0 + DEFAULT_TTL_SECS, Ordering::SeqCst);
    assert_eq!(t.sweep().unwrap(), 1);
    assert_eq!(t.sweep().unwrap(), 0);
}

/// **A declined ticket is spent and listed as declined**, through Postgres:
/// it cannot be redeemed or declined again, it leaves the open list, it is
/// on the declined list with the instant it was declined until it would
/// have expired, and the wrong holder neither declines nor spends it.
#[tokio::test(flavor = "multi_thread")]
async fn the_pg_store_keeps_a_decline_until_the_ticket_would_have_expired() {
    let db = "ordo_tickets_decline";
    let _pool = prepared(db).await;
    let store: Arc<dyn TicketStore> = Arc::new(PgTicketStore::open(pool_config(db)).expect("s"));
    let (t, clock) = tickets(store);
    let mut s = spec(Purpose::Invitation);
    s.space = Some("spc_a".into());
    let issued = t.issue(s).unwrap();

    let thief = Want {
        purpose: Some(Purpose::Invitation),
        holder: Some("usr_thief"),
        ..Default::default()
    };
    assert_eq!(
        t.decline(&issued.ticket, &thief).err(),
        Some(TicketError::Refused)
    );
    assert_eq!(t.list_open("spc_a", Purpose::Invitation).unwrap().len(), 1);

    clock.store(T0 + 60, Ordering::SeqCst);
    let ana = Want {
        purpose: Some(Purpose::Invitation),
        holder: Some("usr_ana"),
        ..Default::default()
    };
    let rec = t.decline(&issued.ticket, &ana).expect("declined");
    assert_eq!(rec.space.as_deref(), Some("spc_a"));
    assert_eq!(
        t.decline(&issued.ticket, &ana).err(),
        Some(TicketError::Refused)
    );
    assert_eq!(t.redeem(&issued.ticket), Err(TicketError::Refused));
    assert!(t
        .list_open("spc_a", Purpose::Invitation)
        .unwrap()
        .is_empty());
    let declined = t.list_declined("spc_a", Purpose::Invitation).unwrap();
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].1, T0 + 60);
    assert!(t
        .list_declined("spc_b", Purpose::Invitation)
        .unwrap()
        .is_empty());

    clock.store(T0 + DEFAULT_TTL_SECS, Ordering::SeqCst);
    assert!(t
        .list_declined("spc_a", Purpose::Invitation)
        .unwrap()
        .is_empty());
    assert_eq!(t.sweep().unwrap(), 0, "a declined ticket is not unused");
}

/// **The process-local store keeps a decline as the Postgres one does**: the
/// ticket is spent, the wrong holder neither declines nor spends it, it is
/// listed as declined until it would have expired, and the sweep counts it
/// as used.
#[test]
fn the_memory_store_keeps_a_decline_until_the_ticket_would_have_expired() {
    let (t, clock) = tickets(Arc::new(MemoryTicketStore::new()));
    let mut s = spec(Purpose::Invitation);
    s.space = Some("spc_a".into());
    let issued = t.issue(s).unwrap();
    let thief = Want {
        purpose: Some(Purpose::Invitation),
        holder: Some("usr_thief"),
        ..Default::default()
    };
    assert_eq!(
        t.decline(&issued.ticket, &thief).err(),
        Some(TicketError::Refused)
    );

    clock.store(T0 + 60, Ordering::SeqCst);
    let ana = Want {
        purpose: Some(Purpose::Invitation),
        holder: Some("usr_ana"),
        ..Default::default()
    };
    t.decline(&issued.ticket, &ana).expect("declined");
    assert_eq!(t.redeem(&issued.ticket), Err(TicketError::Refused));
    assert!(t
        .list_open("spc_a", Purpose::Invitation)
        .unwrap()
        .is_empty());
    let declined = t.list_declined("spc_a", Purpose::Invitation).unwrap();
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].1, T0 + 60);

    clock.store(T0 + DEFAULT_TTL_SECS, Ordering::SeqCst);
    assert_eq!(t.sweep().unwrap(), 0, "a declined ticket is not unused");
    assert!(t
        .list_declined("spc_a", Purpose::Invitation)
        .unwrap()
        .is_empty());
}
