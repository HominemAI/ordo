//! **Enrolment E7: what deleting a tenant does to identities** (owner ruling
//! A5, `plan-enrolment.md` §4.6), against real Postgres.
//!
//! - a person the tenant created who ever signed in becomes independent and
//!   keeps their way in;
//! - one who never signed in is removed;
//! - a person who came through the tenant's own provider is destroyed with it,
//!   unless they also hold a platform sign-in, which remains;
//! - another tenant's people are not touched;
//! - "ever signed in" is what `ordo_identity::sign_in` records.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use ordo_conformance::{Fixture, Pg};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::credential_ops::ManagedStore;
use ordo_host::principal_directory::PgDirectoryStore;
use ordo_host::tenant_identities::{NoConnections, TenantConnections, TenantIdentities};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::principal::{AuthMethodKind, PrincipalId, Timestamp};
use ordo_identity::provider::{AuthenticatedSubject, AuthenticatorId};
use ordo_identity::session::{MemorySessionRevocations, SessionRevocations};

const NOW: u64 = 100_000;
const TENANT: &str = "spc_doomed";
const OTHER: &str = "spc_other";

async fn prepared(db: &str) -> Pool {
    let pg = Pg::named(db);
    Fixture::load()
        .expect("fixture")
        .reset(&pg)
        .expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    ordo_host::principal_directory::ensure_table(&pool)
        .await
        .expect("directory");
    ordo_host::credentials::ensure_table(&pool)
        .await
        .expect("credentials");
    ordo_host::tickets::ensure_table(&pool)
        .await
        .expect("tickets");
    ordo_host::credential_ops::ensure_table(&pool)
        .await
        .expect("managed");
    for t in [
        "$authenticator",
        "$principal",
        "$credential",
        "$ticket",
        "$managed",
        "$temporarypassword",
        "$signedin",
    ] {
        pool.rows(&format!("DELETE FROM \"{t}\""))
            .await
            .expect("clean");
    }
    pool
}

/// A principal with one link, and a password credential when the link is one.
async fn person(pool: &Pool, id: &str, link: &str) {
    pool.rows(&format!(
        "INSERT INTO \"$principal\" (\"id\", \"kind\", \"display\", \"created_at\") \
         VALUES ('{id}', 'usr', '{id}', {NOW})"
    ))
    .await
    .expect("principal");
    add_link(pool, id, link).await;
    if let Some(identifier) = link.strip_prefix("password:") {
        pool.rows(&format!(
            "INSERT INTO \"$credential\" (\"identifier\", \"hash\") \
             VALUES ('{identifier}', '$argon2id$fake')"
        ))
        .await
        .expect("credential");
    }
}

async fn add_link(pool: &Pool, id: &str, link: &str) {
    pool.rows(&format!(
        "INSERT INTO \"$authenticator\" (\"authenticator\", \"principal\", \"linked_at\") \
         VALUES ('{link}', '{id}', {NOW})"
    ))
    .await
    .expect("link");
}

async fn managed(pool: &Pool, id: &str, tenant: &str) {
    ManagedStore::new(pool.clone())
        .mark(id, tenant, "mem_owner", NOW)
        .await
        .expect("marked");
}

async fn count(pool: &Pool, table: &str, column: &str, value: &str) -> i64 {
    pool.rows(&format!(
        "SELECT count(*) FROM \"{table}\" WHERE \"{column}\" = '{value}'"
    ))
    .await
    .expect("count")[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

/// What a completed sign-in does, through the one function every sign-in
/// takes, over the real store.
async fn sign_in(pool: &Pool, link: &str) {
    let store = PgDirectoryStore::open_on(pool.clone()).expect("store");
    let link = link.to_string();
    tokio::task::spawn_blocking(move || {
        let mut dir = Directory::with_store(Provisioning::InviteOnly, Arc::new(store));
        let subject =
            AuthenticatedSubject::without_presence(AuthenticatorId(link), AuthMethodKind::Password);
        ordo_identity::sign_in(&mut dir, &subject, "x", Timestamp(NOW)).expect("signed in");
    })
    .await
    .expect("joined");
}

fn settler(
    pool: &Pool,
    connections: Arc<dyn TenantConnections>,
    revocations: Option<Arc<dyn SessionRevocations>>,
) -> (TenantIdentities, Arc<ordo_secevent::Recorder>) {
    let stream = Arc::new(ordo_secevent::Recorder::default());
    (
        TenantIdentities {
            control: pool.clone(),
            connections,
            revocations,
            session_max_secs: 3_600,
            stream: Some(stream.clone()),
            now: Arc::new(|| NOW + 50),
        },
        stream,
    )
}

/// A tenant-scoped provider connection, until F12 builds the real ones.
struct Owns {
    tenant: &'static str,
    connection: &'static str,
    retired: Mutex<Vec<String>>,
}

impl TenantConnections for Owns {
    fn owned_by<'a>(&'a self, tenant: &'a str) -> BoxFuture<'a, Result<Vec<String>, String>> {
        Box::pin(async move {
            Ok(if tenant == self.tenant {
                vec![self.connection.to_string()]
            } else {
                Vec::new()
            })
        })
    }
    fn retire<'a>(&'a self, _: &'a str, connection: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.retired.lock().unwrap().push(connection.to_string());
            Ok(())
        })
    }
}

fn rendered(stream: &ordo_secevent::Recorder) -> String {
    stream
        .take_rendered(ordo_commandlog::Timestamp(NOW + 10_000))
        .join("\n")
}

/// **Named test 1.** A managed identity that signed in survives as
/// independent, on the record, with its way in.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_tenants_managed_identity_that_signed_in_survives_as_independent() {
    let pool = prepared("ordo_e7_survives").await;
    person(&pool, "usr_ann", "password:ann@example.test").await;
    managed(&pool, "usr_ann", TENANT).await;
    sign_in(&pool, "password:ann@example.test").await;

    let (ids, stream) = settler(&pool, Arc::new(NoConnections), None);
    let out = ids
        .after_delete(TENANT, "mem_owner")
        .await
        .expect("settled");

    assert_eq!(out.released, ["usr_ann"]);
    assert!(out.removed.is_empty());
    assert_eq!(count(&pool, "$principal", "id", "usr_ann").await, 1);
    assert_eq!(
        count(&pool, "$authenticator", "principal", "usr_ann").await,
        1,
        "the link stays"
    );
    assert_eq!(
        count(&pool, "$credential", "identifier", "ann@example.test").await,
        1,
        "the credential stays"
    );
    let released: i64 = pool
        .rows("SELECT count(*) FROM \"$managed\" WHERE \"principal\" = 'usr_ann' AND \"released_at\" IS NOT NULL")
        .await
        .unwrap()[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(released, 1, "independent, with the moment recorded");
    let log = rendered(&stream);
    assert!(log.contains("enrol.identity-released"), "{log}");

    // Run again: nothing is done twice.
    let again = ids
        .after_delete(TENANT, "mem_owner")
        .await
        .expect("settled");
    assert_eq!(again, Default::default());
}

/// **Named test 2.** One that never signed in is removed with everything it
/// held.
#[tokio::test(flavor = "multi_thread")]
async fn one_that_never_signed_in_is_removed() {
    let pool = prepared("ordo_e7_removed").await;
    person(&pool, "usr_bea", "password:bea@example.test").await;
    managed(&pool, "usr_bea", TENANT).await;
    pool.rows(
        "INSERT INTO \"$ticket\" (\"hash\", \"principal\", \"purpose\", \"space\", \"role\", \
         \"display\", \"issued_at\", \"expires_at\") VALUES ('h1', 'usr_bea', 'password', \
         'spc_doomed', 'viewer', 'Bea', 1, 999999)",
    )
    .await
    .expect("ticket");

    let (ids, stream) = settler(&pool, Arc::new(NoConnections), None);
    let out = ids
        .after_delete(TENANT, "mem_owner")
        .await
        .expect("settled");

    assert_eq!(out.removed, ["usr_bea"]);
    assert!(out.released.is_empty());
    assert_eq!(count(&pool, "$principal", "id", "usr_bea").await, 0);
    assert_eq!(
        count(&pool, "$authenticator", "principal", "usr_bea").await,
        0
    );
    assert_eq!(
        count(&pool, "$credential", "identifier", "bea@example.test").await,
        0
    );
    assert_eq!(count(&pool, "$ticket", "principal", "usr_bea").await, 0);
    assert_eq!(count(&pool, "$managed", "principal", "usr_bea").await, 0);
    let log = rendered(&stream);
    assert!(log.contains("enrol.identity-removed"), "{log}");
}

/// **Named test 3.** A person who came through the tenant's own provider is
/// destroyed with it, and the same person holding a platform sign-in keeps
/// that. Their old sessions end.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_from_the_tenants_own_provider_is_removed_with_it_and_its_platform_sign_in_remains(
) {
    let pool = prepared("ordo_e7_provider").await;
    person(&pool, "usr_only", "oidc:acme:sub-1").await;
    person(&pool, "usr_both", "oidc:acme:sub-2").await;
    add_link(&pool, "usr_both", "passkey:cred-9").await;
    // A neighbour connection whose name starts the same way is not touched.
    person(&pool, "usr_neighbour", "oidc:acmecorp:sub-3").await;

    let connections = Arc::new(Owns {
        tenant: TENANT,
        connection: "acme",
        retired: Mutex::new(Vec::new()),
    });
    let revocations = Arc::new(MemorySessionRevocations::new());
    let (ids, _) = settler(&pool, connections.clone(), Some(revocations.clone()));
    let out = ids
        .after_delete(TENANT, "mem_owner")
        .await
        .expect("settled");

    assert_eq!(out.removed, ["usr_only"]);
    assert_eq!(out.retired, ["acme"]);
    assert_eq!(*connections.retired.lock().unwrap(), ["acme"]);
    assert_eq!(count(&pool, "$principal", "id", "usr_only").await, 0);

    assert_eq!(count(&pool, "$principal", "id", "usr_both").await, 1);
    assert_eq!(
        count(&pool, "$authenticator", "authenticator", "passkey:cred-9").await,
        1,
        "the platform sign-in remains"
    );
    assert_eq!(
        count(&pool, "$authenticator", "authenticator", "oidc:acme:sub-2").await,
        0,
        "the tenant provider's link went with it"
    );
    assert_eq!(
        count(&pool, "$principal", "id", "usr_neighbour").await,
        1,
        "a connection that shares a prefix is another connection"
    );
    assert!(
        revocations.principal_ended(&PrincipalId("usr_only".into()), Timestamp(NOW)),
        "the destroyed identity's sessions are ended"
    );
    assert!(!revocations.principal_ended(&PrincipalId("usr_both".into()), Timestamp(NOW)));
}

/// P6: what one tenant's deletion does must not reach into another.
#[tokio::test(flavor = "multi_thread")]
async fn another_tenants_people_are_not_touched() {
    let pool = prepared("ordo_e7_other").await;
    person(&pool, "usr_mine", "password:mine@example.test").await;
    person(&pool, "usr_theirs", "password:theirs@example.test").await;
    managed(&pool, "usr_mine", TENANT).await;
    managed(&pool, "usr_theirs", OTHER).await;

    let (ids, _) = settler(&pool, Arc::new(NoConnections), None);
    ids.after_delete(TENANT, "mem_owner")
        .await
        .expect("settled");

    assert_eq!(count(&pool, "$principal", "id", "usr_mine").await, 0);
    assert_eq!(count(&pool, "$principal", "id", "usr_theirs").await, 1);
    assert_eq!(
        count(&pool, "$managed", "principal", "usr_theirs").await,
        1,
        "still managed by the other tenant"
    );
}

/// "Ever signed in" is written by the one function every sign-in passes
/// through, once, at the first.
#[tokio::test(flavor = "multi_thread")]
async fn a_completed_sign_in_is_recorded_once_at_the_first() {
    let pool = prepared("ordo_e7_signedin").await;
    person(&pool, "usr_cat", "password:cat@example.test").await;
    assert_eq!(count(&pool, "$signedin", "principal", "usr_cat").await, 0);
    sign_in(&pool, "password:cat@example.test").await;
    sign_in(&pool, "password:cat@example.test").await;
    assert_eq!(count(&pool, "$signedin", "principal", "usr_cat").await, 1);
    let first = pool
        .rows("SELECT \"first_at\" FROM \"$signedin\" WHERE \"principal\" = 'usr_cat'")
        .await
        .unwrap();
    assert_eq!(first[0][0].as_deref(), Some(NOW.to_string().as_str()));
}
