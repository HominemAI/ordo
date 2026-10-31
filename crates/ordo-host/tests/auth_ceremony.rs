//! The redirect ceremony on the durable table (`plan-auth-federated.md`
//! step 1): `$authceremony` in Postgres, two "processes" that share nothing
//! but the database, and the OAuth2 provider against the stand-in Discord.
//!
//! - a ceremony begun by one process completes in another;
//! - two racing redemptions of one handoff produce one session;
//! - a handoff without its binder is refused, and is not burned by the
//!   attempt;
//! - a connection's endpoints are fixed under its name (§3.5);
//! - the profile store keeps the first link instant and refreshes the rest.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

mod oauth2_support;

use std::sync::Arc;

use axum::http::StatusCode;
use oauth2_support::{begin, callback, nelly, param, redeem, FakeDiscord};
use ordo_host::auth::AuthState;
use ordo_host::ceremonies::PgCeremonyStore;
use ordo_host::federation::{IdentityAttrStore, PgIdentityAttrs};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{
    AuthMethodKind, AuthenticatedSubject, AuthenticatorId, MemorySessionRevocations, PrincipalId,
    ProviderRegistry, SubjectAttributes, Timestamp, TokenKey,
};
use serde_json::json;

const NOW: u64 = 1_800_000_000;

fn pool_config(db: &str) -> ordo_exec::PoolConfig {
    ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    }
}

async fn prepared(db: &str) -> (ordo_exec::Pool, String) {
    let pg = ordo_conformance::Pg::scoped(db);
    let pool = ordo_exec::Pool::connect(pool_config(&pg.db))
        .await
        .expect("pool");
    ordo_host::ceremonies::ensure_table(&pool)
        .await
        .expect("ceremony table");
    ordo_host::federation::ensure_table(&pool)
        .await
        .expect("identity attrs table");
    (pool, pg.db)
}

/// One process: its own ceremony store on its own connection, its own
/// provider and routes. The directory is shared, as the control database is.
fn process(
    db: &str,
    fake: &FakeDiscord,
    store: Arc<ordo_identity::MemoryDirectoryStore>,
) -> axum::Router {
    let ceremonies: Arc<dyn ordo_identity::CeremonyStore> =
        Arc::new(PgCeremonyStore::open(pool_config(db)).expect("ceremony store"));
    let discord = fake.provider("discord", ceremonies.clone());
    let mut registry = ProviderRegistry::new();
    registry
        .register("discord", Box::new(discord.clone()))
        .expect("registered");
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store),
        TokenKey::from_secret(b"auth-ceremony-test"),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_ceremonies(ceremonies)
    .with_oauth2(discord);
    state.now = Box::new(|| NOW);
    ordo_host::auth::router(Some(Arc::new(state)))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ceremony_begun_by_one_process_completes_in_another() {
    let (_pool, db) = prepared("ordo_ceremony_two").await;
    let fake = FakeDiscord::start().await;
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let one = process(&db, &fake, store.clone());
    let two = process(&db, &fake, store);

    // Begun on one, returned to the other by a load balancer, redeemed on the
    // first again.
    let (state, binder) = begin(&one, "discord").await;
    fake.authorize("c1", nelly());
    let location = callback(&two, "discord", &state, "c1").await;
    let handoff = param(&location, "handoff").expect("a handoff");
    let (status, answer) = redeem(&one, "discord", &handoff, &binder, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(answer["token"].is_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn two_racing_completions_produce_one_session() {
    let (_pool, db) = prepared("ordo_ceremony_race").await;
    let fake = FakeDiscord::start().await;
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let one = process(&db, &fake, store.clone());
    let two = process(&db, &fake, store);
    for round in 0..5 {
        let (state, binder) = begin(&one, "discord").await;
        let code = format!("c{round}");
        fake.authorize(&code, nelly());
        let location = callback(&one, "discord", &state, &code).await;
        let handoff = param(&location, "handoff").expect("a handoff");
        let (a, b) = tokio::join!(
            redeem(&one, "discord", &handoff, &binder, json!({})),
            redeem(&two, "discord", &handoff, &binder, json!({})),
        );
        let wins = [a.0, b.0].iter().filter(|s| **s == StatusCode::OK).count();
        assert_eq!(wins, 1, "round {round}: {a:?} {b:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handoff_without_its_binder_is_refused() {
    let (_pool, db) = prepared("ordo_ceremony_binder").await;
    let fake = FakeDiscord::start().await;
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let app = process(&db, &fake, store);
    let (state, binder) = begin(&app, "discord").await;
    fake.authorize("c1", nelly());
    let location = callback(&app, "discord", &state, "c1").await;
    let handoff = param(&location, "handoff").expect("a handoff");
    // The callback URL planted in another browser: that browser has no
    // binder, or a binder for a ceremony of its own.
    let (other_state, other_binder) = begin(&app, "discord").await;
    let _ = other_state;
    for wrong in [String::new(), other_binder] {
        let (status, answer) = redeem(&app, "discord", &handoff, &wrong, json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
        assert!(answer.get("token").is_none());
    }
    let (status, answer) = redeem(&app, "discord", &handoff, &binder, json!({})).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "not burned by the attempts: {answer}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_keeps_its_endpoints_for_life_under_its_name() {
    let (pool, _db) = prepared("ordo_ceremony_fingerprint").await;
    let _ = pool.rows("DELETE FROM \"$authconnection\"").await;
    let fake = FakeDiscord::start().await;
    let conn = fake.connection("discord");
    ordo_host::federation::ensure_fingerprint(&pool, "discord", "oauth2", &conn.fingerprint(), NOW)
        .await
        .expect("first boot records it");
    ordo_host::federation::ensure_fingerprint(&pool, "discord", "oauth2", &conn.fingerprint(), NOW)
        .await
        .expect("the same endpoints boot again");
    let mut moved = fake.connection("discord");
    moved.userinfo_url = "https://evil.example/api/users/@me".into();
    let refused = ordo_host::federation::ensure_fingerprint(
        &pool,
        "discord",
        "oauth2",
        &moved.fingerprint(),
        NOW,
    )
    .await
    .expect_err("other endpoints under the same name refuse the boot");
    assert!(refused.contains("new connection name"), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_profile_store_keeps_the_first_link_and_refreshes_the_rest() {
    let (pool, db) = prepared("ordo_ceremony_attrs").await;
    let _ = pool.rows("DELETE FROM \"$identityattr\"").await;
    let shared = ordo_exec::Pool::connect(pool_config(&db))
        .await
        .expect("pool");
    let store = PgIdentityAttrs::open_on(shared).expect("store");
    let who = PrincipalId("usr_nelly".into());
    let subject = |name: &str| {
        AuthenticatedSubject::without_presence(
            AuthenticatorId("oauth2:discord:1".into()),
            AuthMethodKind::OAuth2,
        )
        .with_attributes(SubjectAttributes {
            display: Some(name.into()),
            ..Default::default()
        })
    };
    let s1 = subject("Nelly");
    let s2 = subject("Nelly O'Neil");
    let (store, who2) = (Arc::new(store), who.clone());
    let st = store.clone();
    tokio::task::spawn_blocking(move || {
        st.note(&who2, &s1, Timestamp(NOW)).expect("noted");
        st.note(&who2, &s2, Timestamp(NOW + 50))
            .expect("noted again");
    })
    .await
    .unwrap();
    let rows = tokio::task::spawn_blocking(move || store.of(&who).expect("read"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].display.as_deref(), Some("Nelly O'Neil"));
    assert_eq!(rows[0].linked_at, NOW);
}

/// Step 7: an open claim is looked at by an emailed code's `begin` and
/// spent by the claim that lands. A look leaves the row, honours phase and
/// expiry, and a later take still wins once.
#[tokio::test(flavor = "multi_thread")]
async fn a_look_at_an_open_ceremony_leaves_it_for_the_take() {
    use ordo_identity::{CeremonyRow, CeremonyStore};
    let (_pool, db) = prepared("ordo_ceremony_peek").await;
    let store = PgCeremonyStore::open(pool_config(&db)).expect("ceremony store");
    let row = CeremonyRow {
        handle: "h-claim".into(),
        connection: "$claim".into(),
        phase: "claim".into(),
        binder: String::new(),
        payload: "{\"principal\":\"usr_kim\"}".into(),
        minted_at: Timestamp(NOW),
        expires_at: Timestamp(NOW + 300),
    };
    tokio::task::spawn_blocking(move || {
        store.put(&row).unwrap();
        let now = Timestamp(NOW + 10);
        assert_eq!(
            store.peek("h-claim", "$claim", "claim", now).unwrap(),
            Some(row.payload.clone())
        );
        assert_eq!(store.peek("h-claim", "$claim", "begun", now).unwrap(), None);
        assert_eq!(
            store
                .peek("h-claim", "$claim", "claim", Timestamp(NOW + 300))
                .unwrap(),
            None,
            "expired reads as absent"
        );
        assert_eq!(
            store.take("h-claim", "$claim", "claim", now).unwrap(),
            Some(row.payload.clone()),
            "the look did not spend it"
        );
        assert_eq!(store.peek("h-claim", "$claim", "claim", now).unwrap(), None);
    })
    .await
    .unwrap();
}
