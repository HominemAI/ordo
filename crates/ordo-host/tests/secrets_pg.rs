//! `$secrets` on the control database (`plan-backend-functions.md` §5).
//! Needs the conformance cluster.
//!
//! What the memory store's unit tests prove, held for the table a
//! deployment runs: a value is sealed at rest, nothing reads it back but the
//! egress kernel's source, a set is a new version, and a removal removes it.

use std::sync::Arc;

use ordo_egress::SecretSource;
use ordo_host::secrets::SecretStore;

const HOME: &str = "ordo_home";
const VALUE: &str = "sk-pg-secret-0123456789";

#[tokio::test(flavor = "multi_thread")]
async fn the_postgres_store_seals_versions_and_removes() {
    let c = ordo_exec::PoolConfig::from_env();
    let db = "ordo_host_t_secrets";
    ordo_exec::drop_database(&c, db).await.expect("drop");
    ordo_exec::create_database(&c, db).await.expect("create");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let store = SecretStore::pg(
        pool.clone(),
        Arc::new(ordo_crypto::TestProvider::with_space(HOME)),
        HOME,
    );
    store.ensure_table().await.expect("table");
    store.ensure_table().await.expect("twice is fine");
    store.bind(
        &ordo_syntax::parse_program("secret openaiKey for api.openai.com\n")
            .unwrap()
            .secrets,
    );

    let one = store
        .set("openaiKey", b"first", "usr_owner", 10)
        .await
        .expect("set");
    assert_eq!(one.version, 1);
    let two = store
        .set("openaiKey", VALUE.as_bytes(), "usr_owner", 20)
        .await
        .expect("set again");
    assert_eq!((two.version, two.set_at), (2, 20));

    // Sealed at rest: the stored bytes do not hold the value.
    let rows = pool
        .rows(r#"SELECT encode("value", 'escape') FROM "$secrets""#)
        .await
        .expect("read the table");
    let at_rest = rows[0][0].clone().unwrap_or_default();
    assert!(!at_rest.contains(VALUE), "stored in the clear: {at_rest}");

    // The listing has names and versions, never a value.
    let listed = store.list().await.expect("list");
    assert_eq!(listed, vec![two.clone()]);
    assert!(!format!("{listed:?}").contains(VALUE));

    // The egress kernel's source is the one way the value comes out.
    assert!(store
        .open("acme", "openaiKey")
        .await
        .expect("open")
        .is_some());
    assert_eq!(
        store.bound_hosts("openaiKey"),
        Some(vec!["api.openai.com".to_string()])
    );

    assert_eq!(store.remove("openaiKey").await.expect("rm"), Some(two));
    assert_eq!(store.remove("openaiKey").await.expect("rm again"), None);
    assert!(store
        .open("acme", "openaiKey")
        .await
        .expect("open")
        .is_none());
    assert!(store.list().await.expect("list").is_empty());
}
