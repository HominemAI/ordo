//! The restart, against real Postgres.
//!
//! `ordo-identity`'s unit tests prove everything a memory store can prove:
//! that the provider reads its credentials through a seam, that the seam is
//! never handed a password, that a logged-out session cannot be refreshed.
//! None of them can prove the thing **20b** actually found, which is that
//! *"credentials live in the provider's in-memory map today, so enrolments
//! do not survive a restart"*.
//!
//! So these tests restart. A store is opened, written to, and then
//! **dropped**, which joins its thread and releases its connection so the
//! process that
//! held the credential is genuinely gone, and a second one is opened
//! against the same database. What the second one can still do is the
//! assertion.
//!
//! Databases are prefixed `ordo_authdur_`. Needs the conformance cluster
//! (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_host::credentials::PgCredentialStore;
use ordo_host::principal_directory::PgDirectoryStore;
use ordo_host::sessions::PgSessionRevocations;
use ordo_identity::directory::{Directory, DirectoryStore, Provisioning};
use ordo_identity::principal::{AuthMethodKind, PrincipalKind};
use ordo_identity::provider::{
    Argon2idHasher, AuthProvider, AuthenticatedSubject, AuthenticatorId, BeginRequest,
    ChallengeOrRedirect,
};
use ordo_identity::{
    CredentialStore, LocalPasswordProvider, SessionClaim, SessionId, SessionRevocations, Timestamp,
};

const NOW: u64 = 1_000_000;

fn pool_config(db: &str) -> ordo_exec::PoolConfig {
    ordo_exec::PoolConfig {
        db: db.to_string(),
        // Two other suites share this cluster; one connection is all a
        // store needs and all it should hold.
        size: 1,
        ..ordo_exec::PoolConfig::from_env()
    }
}

/// A fresh database with the two auth tables and nothing in them. These
/// databases outlive a run, and a previous run's credential is
/// indistinguishable from this one's.
async fn prepared(db: &str) -> ordo_exec::Pool {
    let pg = ordo_conformance::Pg::named(db);
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(pool_config(db))
        .await
        .expect("pool");
    ordo_host::credentials::ensure_table(&pool)
        .await
        .expect("credential table");
    ordo_host::sessions::ensure_table(&pool)
        .await
        .expect("session revocation table");
    ordo_host::principal_directory::ensure_table(&pool)
        .await
        .expect("directory tables");
    ordo_host::passkeys::ensure_table(&pool)
        .await
        .expect("passkey table");
    ordo_host::ceremonies::ensure_table(&pool)
        .await
        .expect("ceremony table");
    let _ = pool.rows("DELETE FROM \"$passkey\"").await;
    let _ = pool.rows("DELETE FROM \"$authceremony\"").await;
    let _ = pool.rows("DELETE FROM \"$credential\"").await;
    let _ = pool.rows("DELETE FROM \"$sessionrevocation\"").await;
    let _ = pool.rows("DELETE FROM \"$authenticator\"").await;
    let _ = pool.rows("DELETE FROM \"$principal\"").await;
    pool
}

fn answer(p: &dyn AuthProvider, identifier: &str, password: &str) -> bool {
    let ChallengeOrRedirect::Challenge { id, .. } = p
        .begin(
            &BeginRequest {
                identifier: Some(identifier.into()),
                return_to: None,
                presence: false,
            },
            Timestamp(NOW),
        )
        .expect("begins")
    else {
        panic!("password is a challenge");
    };
    p.complete(
        &ordo_identity::provider::CompleteRequest::Password {
            challenge: id,
            identifier: identifier.into(),
            password: password.into(),
        },
        Timestamp(NOW),
    )
    .is_ok()
}

/// **The finding, closed.** A credential enrolled through one provider is
/// still there after the store that held it has been dropped and a new one
/// opened against the same database.
///
/// Against the old code this was not a failing test but an impossible one:
/// the credential table was a `BTreeMap` field on the provider, so there
/// was no second process to open and nothing to open it against.
#[tokio::test(flavor = "multi_thread")]
async fn a_credential_survives_a_restart() {
    let db = "ordo_authdur_restart";
    let _pool = prepared(db).await;

    {
        let store = Arc::new(PgCredentialStore::open(pool_config(db)).expect("first store"));
        let provider =
            LocalPasswordProvider::with_store(Argon2idHasher::weak_for_tests(), store.clone());
        provider
            .enroll("ana@example.test", "correct-horse")
            .expect("enrolled");
        assert!(answer(&provider, "ana@example.test", "correct-horse"));
        // The store, its thread and its connection all go here.
    }

    let store = Arc::new(PgCredentialStore::open(pool_config(db)).expect("second store"));
    let provider = LocalPasswordProvider::with_store(Argon2idHasher::weak_for_tests(), store);
    assert!(
        answer(&provider, "ana@example.test", "correct-horse"),
        "the credential did not survive the restart"
    );
    assert!(
        !answer(&provider, "ana@example.test", "not-it"),
        "and it is still a credential rather than a door"
    );
}

/// Nothing stored is a password, and that is checked against the bytes in
/// the table rather than against the store's own report of them.
#[tokio::test(flavor = "multi_thread")]
async fn the_table_holds_no_plaintext() {
    let db = "ordo_authdur_plaintext";
    let pool = prepared(db).await;

    let store = Arc::new(PgCredentialStore::open(pool_config(db)).expect("store"));
    let provider = LocalPasswordProvider::with_store(Argon2idHasher::weak_for_tests(), store);
    provider
        .enroll("ana@example.test", "correct-horse")
        .expect("enrolled");

    // Every column of every row, read straight out of Postgres.
    let rows = pool
        .rows(r#"SELECT "identifier", "hash", "enrolled_at", "updated_at" FROM "$credential""#)
        .await
        .expect("readable");
    assert_eq!(rows.len(), 1);
    let dump = format!("{:?}", rows[0]);
    assert!(
        !dump.contains("correct-horse"),
        "the credential table contains the password: {dump}"
    );
    assert!(
        dump.contains("$argon2id$"),
        "what it should contain is a PHC string: {dump}"
    );
}

/// A password change replaces the credential and keeps the date the account
/// first got one, which is the question an incident asks.
#[tokio::test(flavor = "multi_thread")]
async fn re_enrolling_replaces_the_credential_and_keeps_the_enrolment_date() {
    let db = "ordo_authdur_reenrol";
    let pool = prepared(db).await;

    let store = PgCredentialStore::open_with_clock(pool_config(db), Box::new(|| 111))
        .expect("store at 111");
    store.put("ana@example.test", "$first$").expect("enrolled");
    drop(store);

    let store = PgCredentialStore::open_with_clock(pool_config(db), Box::new(|| 222))
        .expect("store at 222");
    store.put("ana@example.test", "$second$").expect("changed");
    assert_eq!(
        store.hash_for("ana@example.test").expect("readable"),
        Some("$second$".to_string()),
        "a re-enrolment replaces rather than adding a second row"
    );
    drop(store);

    let rows = pool
        .rows(r#"SELECT "enrolled_at", "updated_at" FROM "$credential""#)
        .await
        .expect("readable");
    assert_eq!(rows.len(), 1, "one identifier, one row");
    assert_eq!(rows[0][0].as_deref(), Some("111"), "enrolled_at is kept");
    assert_eq!(rows[0][1].as_deref(), Some("222"), "updated_at moved");
}

/// The store refuses rather than reporting an empty credential table.
///
/// "You have not provisioned this deployment" and "nobody is enrolled" are
/// the same words for a lockout and a first minute, and the second reading
/// looks perfectly healthy while nobody can log in.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_table_is_a_refusal_rather_than_an_empty_one() {
    let db = "ordo_authdur_notable";
    let pool = prepared(db).await;
    pool.rows(r#"DROP TABLE "$credential""#)
        .await
        .expect("dropped");

    let err = PgCredentialStore::open(pool_config(db))
        .err()
        .expect("a store with no table must refuse");
    assert!(
        err.to_string().contains("$credential"),
        "the refusal should name the table: {err}"
    );
}

/// **A logout survives a restart.** The deny list is durable, so a session
/// somebody ended stays ended across a deploy rather than coming back with
/// the next one.
#[tokio::test(flavor = "multi_thread")]
async fn a_logout_survives_a_restart() {
    let db = "ordo_authdur_logout";
    let pool = prepared(db).await;
    let session = SessionId("a1b2c3d4e5f60718293a4b5c6d7e8f90".into());

    {
        let store = PgSessionRevocations::open_with_clock(pool.clone(), Box::new(|| NOW))
            .await
            .expect("first store");
        assert!(!store.is_revoked(&session), "nothing is revoked yet");
        store
            .revoke(&session, Timestamp(NOW + 3_600))
            .expect("ended");
        assert!(store.is_revoked(&session));
    }

    let store = PgSessionRevocations::open_with_clock(pool.clone(), Box::new(|| NOW + 10))
        .await
        .expect("second store");
    assert!(
        store.is_revoked(&session),
        "the logout did not survive the restart"
    );
    assert_eq!(store.len(), 1);
}

/// An entry is swept once every token naming it is dead anyway, which is
/// what keeps the table bounded by logouts rather than growing for the life
/// of the deployment.
#[tokio::test(flavor = "multi_thread")]
async fn a_revocation_is_swept_at_the_session_ceiling() {
    let db = "ordo_authdur_sweep";
    let pool = prepared(db).await;
    let session = SessionId("00112233445566778899aabbccddeeff".into());
    let ceiling = NOW + 3_600;

    let store = PgSessionRevocations::open_with_clock(pool.clone(), Box::new(|| NOW))
        .await
        .expect("store");
    store.revoke(&session, Timestamp(ceiling)).expect("ended");

    // A second store whose clock is past the ceiling: its load is also the
    // sweep, and the row is gone from the table rather than only from the
    // list.
    let later = PgSessionRevocations::open_with_clock(pool.clone(), Box::new(move || ceiling))
        .await
        .expect("store, later");
    assert!(!later.is_revoked(&session));
    assert_eq!(later.len(), 0);
    let rows = pool
        .rows(r#"SELECT "session_id" FROM "$sessionrevocation""#)
        .await
        .expect("readable");
    assert!(rows.is_empty(), "the row itself is swept, not just hidden");
}

/// Two replicas, one table. A logout on one is seen by the other after a
/// refresh, which is the staleness `PgSessionRevocations` names and the
/// cadence that closes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_replica_sees_a_logout_after_a_refresh() {
    let db = "ordo_authdur_replicas";
    let pool = prepared(db).await;
    let session = SessionClaim::starting_at(Timestamp(NOW));

    let one = PgSessionRevocations::open_with_clock(pool.clone(), Box::new(|| NOW))
        .await
        .expect("replica one");
    let two = PgSessionRevocations::open_with_clock(pool.clone(), Box::new(|| NOW))
        .await
        .expect("replica two");

    one.revoke(&session.id, session.ceiling(43_200))
        .expect("ended on replica one");
    assert!(one.is_revoked(&session.id), "the replica that wrote it");
    assert!(
        !two.is_revoked(&session.id),
        "and the other one has not looked yet, which is the stated window"
    );

    two.refresh().await.expect("re-read");
    assert!(
        two.is_revoked(&session.id),
        "after the cadence, both replicas refuse it"
    );
}

// ───────────────── the directory (20b, closed 2026-09-20) ────────────────

fn subject(authenticator: &str) -> AuthenticatedSubject {
    AuthenticatedSubject::without_presence(
        AuthenticatorId(authenticator.to_string()),
        AuthMethodKind::Password,
    )
}

/// **The finding, closed.** Which authenticator belongs to which principal
/// used to live in `Directory`'s own `BTreeMap`, so a restart forgot every
/// link. An authenticator enrolled through one directory still resolves to
/// its principal after the store that held it has been dropped (which
/// joins its thread and releases its connection) and a fresh directory is
/// opened against the same database.
#[tokio::test(flavor = "multi_thread")]
async fn an_enrolled_authenticator_survives_a_restart() {
    let db = "ordo_authdur_directory_restart";
    let _pool = prepared(db).await;

    let principal = {
        let store = Arc::new(PgDirectoryStore::open(pool_config(db)).expect("first store"));
        let mut directory = Directory::with_store(Provisioning::InviteOnly, store);
        let p = directory
            .create_principal(PrincipalKind::Human, "Ana", Timestamp(NOW))
            .expect("minted");
        directory
            .link_first(
                &p,
                &AuthenticatorId("password:ana@example.test".into()),
                Timestamp(NOW),
            )
            .expect("linked");
        p
        // The store, its thread and its connection all go here.
    };

    let store = Arc::new(PgDirectoryStore::open(pool_config(db)).expect("second store"));
    let directory = Directory::with_store(Provisioning::InviteOnly, store);
    assert_eq!(
        directory
            .principal_for(&AuthenticatorId("password:ana@example.test".into()))
            .expect("readable"),
        Some(principal.clone()),
        "the link did not survive the restart"
    );
    assert_eq!(
        directory
            .principal(&principal)
            .expect("readable")
            .map(|p| p.kind),
        Some(PrincipalKind::Human),
        "the principal itself did not survive the restart"
    );
}

/// **Auto-onboard is idempotent across a restart.** The old code re-minted
/// principal ids from a counter that also restarted, so a second login from
/// the same unrecognised authenticator after a restart onboarded a *second*
/// principal under the first one's identifier — a collision, not a
/// lockout. The durable link means `resolve_or_onboard` finds the row
/// before it ever reaches minting.
#[tokio::test(flavor = "multi_thread")]
async fn auto_onboard_twice_across_a_restart_yields_the_same_principal() {
    let db = "ordo_authdur_directory_onboard";
    let _pool = prepared(db).await;
    let who = subject("oidc:google:ana");

    let first = {
        let store = Arc::new(PgDirectoryStore::open(pool_config(db)).expect("first store"));
        let mut directory = Directory::with_store(Provisioning::AutoOnboard, store);
        let (p, events) = directory
            .resolve_or_onboard(&who, "Ana", Timestamp(NOW))
            .expect("onboarded");
        assert_eq!(events.len(), 2, "onboarded and linked, both audited");
        p
    };

    let store = Arc::new(PgDirectoryStore::open(pool_config(db)).expect("second store"));
    let mut directory = Directory::with_store(Provisioning::AutoOnboard, store);
    let (second, events) = directory
        .resolve_or_onboard(&who, "Ana", Timestamp(NOW + 10))
        .expect("resolved");
    assert_eq!(
        second, first,
        "a second onboard across a restart minted a second principal"
    );
    assert!(
        events.is_empty(),
        "resolving an existing link mints nothing and audits nothing"
    );
}

/// A principal id cannot be minted that collides with an existing one. The
/// store's id draw is rigged to repeat the same suffix, so the second
/// `create_principal` must find `$principal` already holding that id and
/// draw again — proving the retry runs against Postgres's own constraint
/// rather than trusting a 128-bit draw never collides in practice.
#[tokio::test(flavor = "multi_thread")]
async fn a_principal_id_cannot_be_minted_that_collides_with_an_existing_one() {
    let db = "ordo_authdur_directory_collision";
    let pool = prepared(db).await;

    let draws = std::sync::Mutex::new(vec!["second", "dup", "dup"]);
    let store = PgDirectoryStore::open_with_id_source(
        pool_config(db),
        Box::new(move || {
            draws
                .lock()
                .unwrap()
                .pop()
                .expect("no more rigged draws")
                .to_string()
        }),
    )
    .expect("store");

    let first = store
        .create_principal(PrincipalKind::Human, "A", Timestamp(NOW))
        .expect("first mint");
    assert_eq!(first.0, "usr_dup");

    // Draws "dup" again (collision with `first` in `$principal`), then
    // "second".
    let second = store
        .create_principal(PrincipalKind::Human, "B", Timestamp(NOW))
        .expect("second mint");
    assert_eq!(second.0, "usr_second");
    assert_ne!(first, second, "a collision must never be minted");

    let rows = pool
        .rows(r#"SELECT "id" FROM "$principal" ORDER BY "id""#)
        .await
        .expect("readable");
    assert_eq!(
        rows.len(),
        2,
        "exactly two principals exist, not a collision merged into one: {rows:?}"
    );
}

// ══════════════════════ passkeys and ceremonies survive ══════════════════

use ordo_host::ceremonies::PgCeremonyStore;
use ordo_host::passkeys::PgPasskeyStore;
use ordo_identity::ceremony::{CeremonyRow, CeremonyStore};
use ordo_identity::passkey::{PasskeyStore, PasskeyStoreError};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::{AuthError, PasskeyProvider, RelyingParty, WebAuthnError};

fn relying_party() -> RelyingParty {
    RelyingParty::new("example.com", &["https://app.example.com"]).expect("a relying party")
}

/// A provider over stores it opened itself. Dropping it is the restart:
/// each store joins its thread and releases its connection.
fn passkey_provider(db: &str) -> PasskeyProvider {
    PasskeyProvider::with_stores(
        relying_party(),
        Arc::new(PgPasskeyStore::open(pool_config(db)).expect("passkey store")),
        Arc::new(PgCeremonyStore::open(pool_config(db)).expect("ceremony store")),
    )
}

fn assertion_challenge(p: &PasskeyProvider) -> String {
    p.begin_assertion(Timestamp(NOW), &[])
        .expect("a challenge")
        .challenge
}

fn registration_challenge(p: &PasskeyProvider) -> String {
    p.begin_registration(Timestamp(NOW), &[])
        .expect("options")
        .challenge
}

/// **A registered passkey and an open ceremony survive a restart.** The
/// public key was a `BTreeMap` field on the provider, and so was the
/// challenge: there was no second process to open and nothing to open it
/// against. The challenge here is minted **before** the restart and answered
/// **after** it.
#[tokio::test(flavor = "multi_thread")]
async fn a_passkey_survives_a_restart() {
    let db = "ordo_authdur_passkey";
    let _pool = prepared(db).await;
    let device = TestAuthenticator::es256();

    let open_challenge = {
        let p = passkey_provider(db);
        let ch = registration_challenge(&p);
        p.complete(&device.register(&ch), Timestamp(NOW))
            .expect("registers");
        assertion_challenge(&p)
    };

    let p = passkey_provider(db);
    let subject = p
        .complete(&device.assert(&open_challenge), Timestamp(NOW + 1))
        .expect("the passkey and the open challenge both survived");
    assert_eq!(subject.authenticator, device.authenticator_id());
    assert!(subject.presence.is_some());
    let stored = p
        .stored(&device.credential_id)
        .expect("readable")
        .expect("stored");
    assert_eq!(stored.credential.key().to_bytes().len(), 65);
    assert_eq!(stored.user_handle.len(), 32);
    assert_eq!(stored.registered_at, Timestamp(NOW));
    assert_eq!(stored.last_used_at, Timestamp(NOW + 1));
}

/// **The sign counter is written through**, so the clone check is not
/// something a restart resets.
#[tokio::test(flavor = "multi_thread")]
async fn a_sign_count_regression_is_detected_after_a_restart() {
    let db = "ordo_authdur_signcount";
    let _pool = prepared(db).await;
    let mut device = TestAuthenticator::es256();

    {
        let p = passkey_provider(db);
        let ch = registration_challenge(&p);
        p.complete(&device.register(&ch), Timestamp(NOW))
            .expect("registers");
        device.sign_count = 5;
        let ch = assertion_challenge(&p);
        p.complete(&device.assert(&ch), Timestamp(NOW))
            .expect("count 5 is fresh");
    }

    let p = passkey_provider(db);
    // The same count, and a lower one, are a clone: an authenticator that
    // answered 5 does not answer 5 again.
    for replayed in [5, 3] {
        device.sign_count = replayed;
        let ch = assertion_challenge(&p);
        assert_eq!(
            p.complete(&device.assert(&ch), Timestamp(NOW)),
            Err(AuthError::WebAuthn(WebAuthnError::SignCountRegression {
                stored: 5,
                presented: replayed
            })),
            "count {replayed} after a restart"
        );
    }
    device.sign_count = 6;
    let ch = assertion_challenge(&p);
    assert!(p.complete(&device.assert(&ch), Timestamp(NOW)).is_ok());
}

/// The backup flags and the user handle reach the row, and a second
/// registration of the same credential id is refused by the database.
#[tokio::test(flavor = "multi_thread")]
async fn the_row_keeps_the_backup_flags_and_refuses_a_second_registration() {
    let db = "ordo_authdur_flags";
    let pool = prepared(db).await;
    let p = passkey_provider(db);
    let synced = TestAuthenticator {
        backup_eligible: true,
        backed_up: true,
        ..TestAuthenticator::es256()
    };
    let ch = registration_challenge(&p);
    p.complete(&synced.register(&ch), Timestamp(NOW))
        .expect("registers");

    let rows = pool
        .rows(
            "SELECT \"backup_eligible\"::int, \"backed_up\"::int, \"algorithm\" \
             FROM \"$passkey\"",
        )
        .await
        .expect("read");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].as_deref(), Some("1"));
    assert_eq!(rows[0][1].as_deref(), Some("1"));
    assert_eq!(rows[0][2].as_deref(), Some("-7"));

    let again = registration_challenge(&p);
    assert_eq!(
        p.complete(&synced.register(&again), Timestamp(NOW)),
        Err(AuthError::CredentialAlreadyEnrolled)
    );
    let stored = p.stored(&synced.credential_id).unwrap().expect("stored");
    assert!(stored.backup_eligible && stored.backed_up);
}

/// A row whose key is not a point on the curve does not load, and is a
/// store error rather than a credential.
#[tokio::test(flavor = "multi_thread")]
async fn a_hand_edited_key_does_not_load() {
    let db = "ordo_authdur_badkey";
    let pool = prepared(db).await;
    let p = passkey_provider(db);
    let device = TestAuthenticator::es256();
    let ch = registration_challenge(&p);
    p.complete(&device.register(&ch), Timestamp(NOW))
        .expect("registers");
    let mut bad = vec![0x04u8];
    bad.extend_from_slice(&[1u8; 64]);
    let bad = ordo_identity::webauthn::b64url_encode(&bad);
    pool.rows(&format!("UPDATE \"$passkey\" SET \"public_key\" = '{bad}'"))
        .await
        .expect("edit");
    let store = PgPasskeyStore::open(pool_config(db)).expect("store");
    assert!(matches!(
        store.get(&device.credential_id),
        Err(PasskeyStoreError::Unavailable(_))
    ));
}

/// **One-shot across two stores**: the delete is the database's, so a
/// challenge taken through one connection is gone through the other, and an
/// expired one answers nothing and is swept.
#[tokio::test(flavor = "multi_thread")]
async fn a_ceremony_is_taken_once_across_two_stores_and_expires() {
    let db = "ordo_authdur_ceremony";
    let _pool = prepared(db).await;
    let one = PgCeremonyStore::open(pool_config(db)).expect("one");
    let two = PgCeremonyStore::open(pool_config(db)).expect("two");
    let row = |handle: &str, expires: u64| CeremonyRow {
        handle: handle.into(),
        connection: "passkey".into(),
        phase: "assert".into(),
        binder: String::new(),
        payload: "{\"k\":\"it's\"}".into(),
        minted_at: Timestamp(NOW),
        expires_at: Timestamp(expires),
    };
    let now = Timestamp(NOW + 10);
    one.put(&row("h1", NOW + 300)).expect("put");
    // The wrong phase and the wrong connection do not take it.
    assert_eq!(two.take("h1", "passkey", "register", now), Ok(None));
    assert_eq!(two.take("h1", "other", "assert", now), Ok(None));
    assert_eq!(
        two.take("h1", "passkey", "assert", now),
        Ok(Some("{\"k\":\"it's\"}".into())),
        "the payload comes back exactly"
    );
    assert_eq!(one.take("h1", "passkey", "assert", now), Ok(None));

    one.put(&row("h2", NOW + 20)).expect("put");
    assert_eq!(
        one.take("h2", "passkey", "assert", Timestamp(NOW + 20)),
        Ok(None),
        "expired at its own instant"
    );
    assert_eq!(two.sweep(Timestamp(NOW + 20)), Ok(1));
    assert_eq!(two.sweep(Timestamp(NOW + 20)), Ok(0));
}

/// `authenticators_of` lists exactly a principal's links, sorted, in the
/// database and in the memory double alike.
#[tokio::test(flavor = "multi_thread")]
async fn authenticators_of_lists_one_principals_links_sorted() {
    use ordo_identity::MemoryDirectoryStore;
    let db = "ordo_authdur_authenticators_of";
    let _pool = prepared(db).await;
    let pg: Arc<dyn DirectoryStore> =
        Arc::new(PgDirectoryStore::open(pool_config(db)).expect("pg"));
    let memory: Arc<dyn DirectoryStore> = Arc::new(MemoryDirectoryStore::new());
    for store in [pg, memory] {
        let mut principals = Vec::new();
        for name in ["Ana", "Ben"] {
            let id = store
                .create_principal(PrincipalKind::Human, name, Timestamp(NOW))
                .expect("minted");
            principals.push(store.principal(&id).expect("read").expect("exists"));
        }
        for (i, a) in ["passkey:zzz", "password:ana", "passkey:aaa"]
            .iter()
            .enumerate()
        {
            let owner = if i == 1 {
                &principals[1]
            } else {
                &principals[0]
            };
            store
                .insert_link(&AuthenticatorId((*a).into()), owner, Timestamp(NOW))
                .expect("linked");
        }
        let ana: Vec<String> = store
            .authenticators_of(&principals[0].id)
            .expect("listed")
            .into_iter()
            .map(|a| a.0)
            .collect();
        assert_eq!(ana, ["passkey:aaa", "passkey:zzz"]);
        let ben = store.authenticators_of(&principals[1].id).expect("listed");
        assert_eq!(ben, [AuthenticatorId("password:ana".into())]);
        let nobody = store
            .authenticators_of(&ordo_identity::PrincipalId("usr_none".into()))
            .expect("listed");
        assert!(nobody.is_empty());
    }
}

// ───────────────── recovery R1: the epoch survives a restart ─────────────

/// "Sign out everywhere" is a row, not a memory: a deploy after a recovery
/// must not bring a thief's session back.
#[tokio::test(flavor = "multi_thread")]
async fn an_epoch_survives_a_restart() {
    let db = "ordo_authdur_epoch";
    let pool = prepared(db).await;
    let _ = pool.rows("DELETE FROM \"$principalepoch\"").await;
    let who = ordo_identity::PrincipalId("prn_epoch".into());
    let old = Timestamp(NOW - 100);
    let new = Timestamp(NOW + 100);

    {
        let store = PgSessionRevocations::open_with_clock(pool.clone(), Box::new(|| NOW))
            .await
            .expect("first store");
        assert!(!store.principal_ended(&who, old));
        store
            .end_all(&who, Timestamp(NOW), Timestamp(NOW + 43_200))
            .expect("ended");
        assert!(store.principal_ended(&who, old));
    }

    let store = PgSessionRevocations::open_with_clock(pool.clone(), Box::new(|| NOW + 10))
        .await
        .expect("second store");
    assert!(
        store.principal_ended(&who, old),
        "the epoch did not survive the restart"
    );
    assert!(
        !store.principal_ended(&who, new),
        "a session that starts after it is not ended"
    );
}

// ───────────────── recovery R3: the case and the link survive ────────────

/// A case is a row: it survives a restart, a person holds one live case
/// (the database's unique index, not a check), and the transitions are
/// atomic.
#[tokio::test(flavor = "multi_thread")]
async fn a_case_survives_a_restart_and_a_person_has_one_live_case() {
    use ordo_host::recovery::{Case, InsertError, Path, PgRecoveryStore, RecoveryStore, State};
    let db = "ordo_authdur_recovery";
    let pool = prepared(db).await;
    ordo_host::recovery::ensure_table(&pool)
        .await
        .expect("recovery table");
    let _ = pool.rows("DELETE FROM \"$recovery\"").await;
    let case = |id: &str, principal: &str, state: State| Case {
        id: id.into(),
        principal: principal.into(),
        path: Path::Host,
        opened_by: "adam".into(),
        authority: "host".into(),
        evidence: "host-break-glass".into(),
        reference: "T-1".into(),
        opened_at: NOW,
        matures_at: NOW,
        contest_until: 0,
        state,
        space: String::new(),
        acknowledged: false,
        approvals_required: 1,
        departed: String::new(),
    };
    {
        let store = PgRecoveryStore::open_on(pool.clone()).expect("first store");
        store
            .insert(&case("rec_1", "prn_a", State::Matured))
            .expect("inserted");
        assert_eq!(
            store.insert(&case("rec_2", "prn_a", State::Matured)),
            Err(InsertError::Live),
            "one live case per person, by the index"
        );
        store
            .insert(&case("rec_3", "prn_b", State::Matured))
            .expect("another person is another case");
    }
    let store = PgRecoveryStore::open_on(pool.clone()).expect("second store");
    let got = store.get("rec_1").expect("read").expect("kept");
    assert_eq!(got.state, State::Matured);
    assert_eq!(got.reference, "T-1");
    assert_eq!(
        store.live_for("prn_a").expect("read").map(|c| c.id),
        Some("rec_1".to_string())
    );

    // A transition is one compare-and-set: the second of two loses.
    assert!(store
        .transition(
            "rec_1",
            &[State::Matured],
            State::Completed,
            Some(NOW + 100)
        )
        .expect("moved"));
    assert!(!store
        .transition(
            "rec_1",
            &[State::Matured],
            State::Completed,
            Some(NOW + 100)
        )
        .expect("refused"));
    // A completed case is live until its window is over, then it closes and
    // frees the person for another.
    assert!(store.close_expired(NOW + 99).expect("swept").is_empty());
    assert_eq!(store.close_expired(NOW + 100).expect("swept").len(), 1);
    store
        .insert(&case("rec_4", "prn_a", State::Matured))
        .expect("free again");
}

/// A succession's departed owners and a manager's objection are rows: they
/// survive a restart, one manager objects to a case once, and the deployment
/// owner's resolution is written on every open objection at once.
#[tokio::test(flavor = "multi_thread")]
async fn an_objection_and_its_resolution_survive_a_restart() {
    use ordo_host::recovery::{
        Case, Objection, Path, PgRecoveryStore, RecoveryStore, Resolution, State,
    };
    let db = "ordo_authdur_objection";
    let pool = prepared(db).await;
    ordo_host::recovery::ensure_table(&pool)
        .await
        .expect("recovery table");
    let _ = pool.rows("DELETE FROM \"$recovery\"").await;
    let _ = pool.rows("DELETE FROM \"$recoveryobjection\"").await;
    let objection = |who: &str| Objection {
        case: "rec_s".into(),
        objector: who.into(),
        space: "spc_t".into(),
        at: NOW,
        resolved_at: 0,
        resolution: String::new(),
        reason: String::new(),
        resolved_by: String::new(),
    };
    {
        let store = PgRecoveryStore::open_on(pool.clone()).expect("first store");
        store
            .insert(&Case {
                id: "rec_s".into(),
                principal: "prn_bob".into(),
                path: Path::Succession,
                opened_by: "adam".into(),
                authority: "deployment-owner".into(),
                evidence: "contract-holder".into(),
                reference: String::new(),
                opened_at: NOW,
                matures_at: NOW + 86_400,
                contest_until: 0,
                state: State::Pending,
                space: "spc_t".into(),
                acknowledged: false,
                approvals_required: 1,
                departed: "prn_gone,prn_also".into(),
            })
            .expect("inserted");
        assert!(store.add_objection(&objection("prn_mia")).expect("added"));
        assert!(
            !store.add_objection(&objection("prn_mia")).expect("read"),
            "one objection per manager"
        );
        assert!(store.add_objection(&objection("prn_max")).expect("added"));
    }
    let store = PgRecoveryStore::open_on(pool.clone()).expect("second store");
    let open = store.open_cases().expect("read");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].departed, "prn_gone,prn_also");
    assert_eq!(store.objections("rec_s").expect("read").len(), 2);
    assert_eq!(
        store
            .resolve_objections(
                "rec_s",
                Resolution::Resume,
                "objector-answered",
                "adam",
                NOW + 5
            )
            .expect("resolved"),
        2
    );
    assert_eq!(
        store
            .resolve_objections(
                "rec_s",
                Resolution::Cancel,
                "opened-in-error",
                "adam",
                NOW + 6
            )
            .expect("nothing left"),
        0,
        "a resolution is written once"
    );
    let all = store.objections("rec_s").expect("read");
    assert!(all.iter().all(|o| o.resolution == "resume"
        && o.reason == "objector-answered"
        && o.resolved_by == "adam"
        && o.resolved_at == NOW + 5));
}

/// What a recovery writes to `$authenticator` survives a restart: the
/// quarantine, the case behind the link, and the retirement with its end.
#[tokio::test(flavor = "multi_thread")]
async fn a_retired_link_and_a_quarantine_survive_a_restart() {
    use ordo_identity::Retired;
    let db = "ordo_authdur_retired";
    let pool = prepared(db).await;
    let old = AuthenticatorId("passkey:old".into());
    let new = AuthenticatorId("passkey:new".into());
    let principal;
    {
        let store = PgDirectoryStore::open_on(pool.clone()).expect("first store");
        principal = store
            .create_principal(PrincipalKind::Human, "Ana", Timestamp(NOW))
            .expect("minted");
        let p = store.principal(&principal).expect("read").expect("exists");
        store.insert_link(&old, &p, Timestamp(NOW)).expect("linked");
        store
            .insert_link_marked(&new, &p, Timestamp(NOW), NOW + 86_400, "rec_1")
            .expect("marked");
        store
            .set_retired(
                &old,
                Some(&Retired {
                    case: "rec_1".into(),
                    until: NOW + 259_200,
                }),
            )
            .expect("retired");
    }
    let store = PgDirectoryStore::open_on(pool.clone()).expect("second store");
    let meta = store.link_meta(&new).expect("read").expect("linked");
    assert_eq!(meta.presence_not_before, NOW + 86_400);
    assert_eq!(meta.recovered_case.as_deref(), Some("rec_1"));
    assert!(meta.retired.is_none());
    let meta = store.link_meta(&old).expect("read").expect("linked");
    assert_eq!(meta.retired.map(|r| r.until), Some(NOW + 259_200));
    assert_eq!(meta.presence_not_before, 0, "an ordinary link reads as one");
    assert_eq!(store.links_of(&principal).expect("read").len(), 2);
    assert!(store.retired_by(NOW + 259_199).expect("read").is_empty());
    let due = store.retired_by(NOW + 259_200).expect("read");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].0, old);
    store.set_retired(&old, None).expect("restored");
    assert!(store
        .link_meta(&old)
        .expect("read")
        .expect("linked")
        .retired
        .is_none());
}

// ───────────────── recovery R4: the codes survive, and spend once ────────

/// A set survives a restart as hashes, replacing a set voids the old one, and
/// a code is spent once even when two replicas race for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_is_spent_once_across_two_stores_and_a_set_survives_a_restart() {
    use ordo_host::recovery_codes::{CodeStore, PgCodeStore};
    let db = "ordo_authdur_codes";
    let pool = prepared(db).await;
    ordo_host::recovery_codes::ensure_table(&pool)
        .await
        .expect("code table");
    let _ = pool.rows("DELETE FROM \"$recoverycode\"").await;
    let hashes: Vec<String> = (0..10).map(|i| format!("hash-{i:02}")).collect();
    {
        let store = PgCodeStore::open_on(pool.clone()).expect("first store");
        store
            .replace_set("prn_a", &hashes, NOW)
            .expect("a set is stored");
        assert_eq!(store.remaining("prn_a").expect("count"), 10);
    }
    let one = Arc::new(PgCodeStore::open_on(pool.clone()).expect("second store"));
    let two = Arc::new(PgCodeStore::open_on(pool.clone()).expect("third store"));
    assert_eq!(one.remaining("prn_a").expect("count"), 10, "kept");
    assert_eq!(
        one.find("hash-03").expect("read").as_deref(),
        Some("prn_a"),
        "found by hash"
    );

    // Two replicas, one code: exactly one of them gets it.
    let (a, b) = (one.clone(), two.clone());
    let ta = std::thread::spawn(move || a.consume("hash-03", "recovery", NOW + 1));
    let tb = std::thread::spawn(move || b.consume("hash-03", "recovery", NOW + 1));
    let got: Vec<Option<String>> = vec![
        ta.join().expect("thread").expect("store"),
        tb.join().expect("thread").expect("store"),
    ];
    assert_eq!(
        got.iter().filter(|g| g.is_some()).count(),
        1,
        "one winner: {got:?}"
    );
    assert_eq!(one.remaining("prn_a").expect("count"), 9);
    assert_eq!(
        one.find("hash-03").expect("read"),
        None,
        "a used code is gone"
    );

    // Voiding deletes what is unused and nothing of another person's.
    one.replace_set("prn_b", &["other".to_string()], NOW)
        .expect("another person");
    assert_eq!(one.void("prn_a").expect("voided"), 9);
    assert_eq!(one.remaining("prn_a").expect("count"), 0);
    assert_eq!(one.remaining("prn_b").expect("count"), 1);

    // A new set replaces the old one whole, used codes included.
    one.replace_set("prn_a", &["fresh".to_string()], NOW + 2)
        .expect("replaced");
    assert_eq!(one.remaining("prn_a").expect("count"), 1);
    assert_eq!(one.find("hash-04").expect("read"), None);
}

/// Renewals (owner ruling 2026-10-21) against real Postgres: a secret is
/// spent once, a reuse past the grace ends the family, and ending a person
/// ends every family they hold. Only hashes are stored.
#[tokio::test(flavor = "multi_thread")]
async fn renewals_rotate_detect_reuse_and_end_in_postgres() {
    use ordo_host::renewal::{hash_of, Family, PgRenewals, RenewalStore, Turn};
    let db = "ordo_authdur_renewal";
    let pool = prepared(db).await;
    ordo_host::renewal::ensure_table(&pool)
        .await
        .expect("renewal table");
    let _ = pool.rows("DELETE FROM \"$renewal\"").await;
    let store = Arc::new(PgRenewals::new(pool.clone()));
    let fam = |id: &str| Family {
        family: id.into(),
        principal: "prn_ana".into(),
        via: Some("passkey".into()),
        second_step: None,
        space: Some("spc_ana".into()),
        signed_in_at: NOW,
    };
    let s = store.clone();
    let turned = tokio::task::spawn_blocking(move || {
        s.issue(&fam("f1"), &hash_of("one"), NOW + 600).unwrap();
        s.issue(&fam("f2"), &hash_of("other"), NOW + 600).unwrap();
        let first = s
            .turn(&hash_of("one"), &hash_of("two"), NOW + 10, 600)
            .unwrap();
        let grace = s
            .turn(&hash_of("one"), &hash_of("x"), NOW + 15, 600)
            .unwrap();
        let reused = s
            .turn(&hash_of("one"), &hash_of("y"), NOW + 100, 600)
            .unwrap();
        let after = s
            .turn(&hash_of("two"), &hash_of("z"), NOW + 101, 600)
            .unwrap();
        let other = s
            .turn(&hash_of("other"), &hash_of("o2"), NOW + 102, 600)
            .unwrap();
        s.end_principal("prn_ana").unwrap();
        let ended = s
            .turn(&hash_of("o2"), &hash_of("o3"), NOW + 103, 600)
            .unwrap();
        (first, grace, reused, after, other, ended)
    })
    .await
    .unwrap();
    assert_eq!(turned.0, Turn::Fresh(fam("f1")));
    assert_eq!(turned.1, Turn::Grace(fam("f1")));
    assert_eq!(turned.2, Turn::Reused(fam("f1")));
    assert_eq!(turned.3, Turn::Unknown, "the reuse ended the family");
    assert_eq!(turned.4, Turn::Fresh(fam("f2")));
    assert_eq!(
        turned.5,
        Turn::Unknown,
        "ending the person ends every family"
    );
    let raw = pool
        .rows("SELECT \"hash\" FROM \"$renewal\"")
        .await
        .unwrap();
    assert!(raw.is_empty(), "{raw:?}");
}

/// The session policy layers (owner, 2026-10-21) against real Postgres: the
/// app developer's layer, a tenant's own, the read, and which tenant a
/// session is for.
#[tokio::test(flavor = "multi_thread")]
async fn session_policy_layers_are_kept_in_postgres() {
    use ordo_host::session_policy::{
        Deployment, SessionPolicies, Source, READ_TENANT, SET_APP, SET_TENANT,
    };
    let db = "ordo_authdur_sessionpolicy";
    let pool = prepared(db).await;
    ordo_host::session_policy::ensure_table(&pool)
        .await
        .expect("policy tables");
    let _ = pool
        .rows("DELETE FROM \"$deploymentsetting\" WHERE \"key\" = 'session_policy'")
        .await;
    let _ = pool.rows("DELETE FROM \"$sessionpolicy\"").await;
    let _ = pool.rows("DELETE FROM \"$sessionspace\"").await;
    let p = SessionPolicies::on_pool(
        Deployment::with_defaults(3_600, 43_200, ordo_host::renewal::RememberConfig::default()),
        pool.clone(),
    )
    .with_home("system");
    let bound = |pairs: &[(&str, &str)]| {
        ordo_api::BoundParams(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
                .collect(),
        )
    };
    p.execute(
        SET_APP,
        "system",
        &bound(&[("tokenSecs", "1800"), ("maxSecsRange", "3600-86400")]),
        "dev",
    )
    .await
    .expect("app layer");
    let e = p
        .execute(SET_APP, "spc_a", &bound(&[("tokenSecs", "1")]), "dev")
        .await
        .expect_err("only in the deployment's own space");
    assert!(
        e.message.contains("deployment's own space"),
        "{}",
        e.message
    );
    p.execute(SET_TENANT, "spc_a", &bound(&[("maxSecs", "7200")]), "owner")
        .await
        .expect("tenant layer");
    let e = p
        .execute(SET_TENANT, "spc_a", &bound(&[("maxSecs", "600")]), "owner")
        .await
        .expect_err("below the app developer's range");
    assert!(e.message.contains("3600-86400"), "{}", e.message);

    let eff = p.effective(Some("spc_a")).await;
    assert_eq!((eff.token_secs, eff.sources[0]), (1_800, Source::App));
    assert_eq!((eff.max_secs, eff.sources[1]), (7_200, Source::Tenant));
    let read = p
        .execute(READ_TENANT, "spc_a", &bound(&[]), "owner")
        .await
        .expect("read");
    assert_eq!(read.rows[0]["maxSecsRange"], "3600-86400");
    assert_eq!(read.rows[0]["tenantChoice"]["maxSecs"], 7_200);

    p.note_session("sess_1", "spc_a", NOW + 100).await;
    assert_eq!(p.session_space("sess_1").await.as_deref(), Some("spc_a"));
    p.sweep(NOW + 101).await.expect("sweep");
    assert_eq!(p.session_space("sess_1").await, None);
    let back = p.readback().await;
    assert_eq!(back["app"]["tokenSecs"], 1_800);
    assert_eq!(back["deployment"]["limits"]["tokenSecs"], "60-86400");
}
