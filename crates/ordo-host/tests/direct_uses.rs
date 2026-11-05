//! `ordo ... --direct` leaves a row; the next boot carries it to the
//! security log (owner, 2026-11-05: "the CLI goes through the door").
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_secevent::EventKind;

const NOW: u64 = 1_800_000_000;

async fn fresh(db: &str) -> ordo_exec::Pool {
    let c = ordo_exec::PoolConfig::from_env();
    ordo_exec::drop_database(&c, db).await.expect("drop");
    ordo_exec::create_database(&c, db).await.expect("create");
    ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 2,
        ..c
    })
    .await
    .expect("pool")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_direct_use_becomes_a_security_event_at_the_next_boot_once() {
    let pool = fresh("ordo_host_t_direct_uses").await;
    ordo_host::direct_uses::record(&pool, "ordo keys init", "ada", "build host", NOW)
        .await
        .expect("recorded");
    ordo_host::direct_uses::record(&pool, "ordo apply", "ada", "build host", NOW + 5)
        .await
        .expect("recorded");

    let stream = ordo_secevent::Recorder::default();
    let n = ordo_host::direct_uses::drain(&pool, &stream)
        .await
        .expect("drained");
    assert_eq!(n, 2);
    let entries = stream.drain_all(ordo_commandlog::Timestamp(NOW + 10));
    let details: Vec<String> = entries
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Deploy { act, detail } if act == ordo_host::direct_uses::ACT => {
                Some(detail.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(details.len(), 2, "{details:?}");
    assert!(
        details[0].starts_with("command=ordo_keys_init by=ada machine=build_host "),
        "{details:?}"
    );
    assert!(details[1].contains("command=ordo_apply"), "{details:?}");

    // The drainer stores them; the rows go then, and the next boot finds
    // nothing to carry.
    let store =
        ordo_exec::secevent::PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    store.try_append_all(&entries).await.expect("stored");
    assert_eq!(
        ordo_host::direct_uses::confirm(&pool)
            .await
            .expect("confirmed"),
        2
    );
    let again = ordo_host::direct_uses::drain(&pool, &stream)
        .await
        .expect("drained");
    assert_eq!(again, 0);
}

/// **A use is not lost when the node dies before the stream is written**
/// (security audit 2026-11-05, L6). The row stays until its event is in
/// the stored stream, so the next boot carries it again.
#[tokio::test(flavor = "multi_thread")]
async fn a_use_whose_event_was_never_stored_is_carried_again() {
    let pool = fresh("ordo_host_t_direct_uses_lost").await;
    ordo_host::direct_uses::record(&pool, "ordo rollback", "ada", "build host", NOW)
        .await
        .expect("recorded");
    // A boot observes it, and dies before its drainer writes.
    let dying = ordo_secevent::Recorder::default();
    assert_eq!(
        ordo_host::direct_uses::drain(&pool, &dying)
            .await
            .expect("drained"),
        1
    );
    drop(dying);
    // The next boot carries it again.
    let next = ordo_secevent::Recorder::default();
    assert_eq!(
        ordo_host::direct_uses::drain(&pool, &next)
            .await
            .expect("drained"),
        1
    );
}
