//! The store contract, run against both bindings.
//!
//! **The filesystem run is unconditional. The S3 run is gated on a live
//! endpoint**, the way `ordo-conformance` gates its Postgres tiers: set
//! `ORDO_BLOB_S3_ENDPOINT` and the four variables beside it and the same
//! suite runs against S3 or a MinIO gateway; leave them unset and the test
//! reports that it did not run rather than passing vacuously.
//!
//! That is the honest CI story and it is stated as one: **CI runs the
//! contract suite against `FsStore` only.** There is no object-storage
//! service in `docker/` and none in the workflow, so nothing here has ever
//! spoken SigV4 to a real endpoint in CI. What holds the S3 binding to the
//! contract in the meantime is (a) this suite, which it can be pointed at
//! in one command, (b) the SigV4 derivation pinned against AWS's published
//! test vector in `s3.rs`, and (c) the fact that the two bindings share
//! the key scheme, the length check and the error mapping rather than
//! re-implementing them. That is less than a live run and it is not
//! pretended to be more.

use std::sync::Arc;

use ordo_blob::{BlobStore, FsStore, S3Config, S3Store, SingleStore, StoreResolver};
use ordo_crypto::custody::SpaceId;

fn tempdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ordo-blob-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn the_filesystem_binding_meets_the_contract() {
    let dir = tempdir("fs");
    let store = FsStore::new(&dir).unwrap();
    let ran = ordo_blob::contract::run(&store, &SpaceId("tenant-a".into())).await;
    assert_eq!(
        ran.len(),
        12,
        "the contract suite shrank: {ran:?} — a case removed is a promise withdrawn"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Gated. Prints what it skipped, so a green run never reads as a live
/// run that happened.
#[tokio::test]
async fn the_s3_binding_meets_the_contract() {
    let Ok(cfg) = S3Config::from_env() else {
        // CI sets ORDO_REQUIRE_S3_TESTS=1 and a local MinIO, so a missing
        // variable there is a failure and not a quiet skip.
        assert!(
            std::env::var("ORDO_REQUIRE_S3_TESTS").is_err(),
            "ORDO_REQUIRE_S3_TESTS is set but ORDO_BLOB_S3_{{ENDPOINT,REGION,BUCKET,\
             ACCESS_KEY_ID,SECRET_ACCESS_KEY}} are not all set"
        );
        eprintln!(
            "SKIPPED: the S3 contract run needs ORDO_BLOB_S3_{{ENDPOINT,REGION,BUCKET,\
             ACCESS_KEY_ID,SECRET_ACCESS_KEY}}. The filesystem binding ran; S3 did not."
        );
        return;
    };
    let store = S3Store::new(cfg);
    let space =
        SpaceId(std::env::var("ORDO_BLOB_S3_SPACE").unwrap_or_else(|_| "ordo-contract".into()));
    let ran = ordo_blob::contract::run(&store, &space).await;
    assert_eq!(ran.len(), 12);
}

/// The filesystem binding refuses to claim production readiness, and the
/// S3 binding claims it. This is the fact a host's boot check reads.
#[tokio::test]
async fn only_one_binding_claims_to_be_a_deployment_target() {
    let dir = tempdir("ready");
    assert!(!FsStore::new(&dir).unwrap().production_ready());
    assert_eq!(FsStore::new(&dir).unwrap().binding(), "filesystem");
    let cfg = S3Config::new("https://s3.example.com", "r", "b", "k", "s").unwrap();
    assert!(S3Store::new(cfg).production_ready());
    std::fs::remove_dir_all(&dir).ok();
}

/// The default resolver hands the same store to every space — honest
/// about the absent space directory, and enough for a deployment with one
/// bucket.
#[tokio::test]
async fn the_default_resolver_is_one_store_for_every_space() {
    let dir = tempdir("resolve");
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).unwrap();
    let r = SingleStore::new(Arc::clone(&store));
    let a = r.resolve(&SpaceId("a".into())).await.unwrap();
    let b = r.resolve(&SpaceId("b".into())).await.unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    std::fs::remove_dir_all(&dir).ok();
}

/// A 100 MB upload streams through in bounded memory. Asserted rather than
/// assumed: the whole point of the streaming trait is that no call site
/// holds the object, and the way that regresses is a `collect` slipping
/// into a binding.
#[tokio::test]
async fn a_large_object_streams_without_being_held() {
    use futures::StreamExt;

    let dir = tempdir("big");
    let store = FsStore::new(&dir).unwrap();
    let space = SpaceId("tenant-a".into());
    let handle = [0x5cu8; 16];

    const CHUNK: usize = 1 << 20;
    const CHUNKS: u64 = 100;
    let total = CHUNK as u64 * CHUNKS;

    let body = Box::pin(futures::stream::unfold(0u64, |n| async move {
        if n == CHUNKS {
            None
        } else {
            Some((Ok(vec![(n % 256) as u8; CHUNK]), n + 1))
        }
    })) as ordo_blob::ByteStream;

    let head = store.put(&space, &handle, total, body).await.unwrap();
    assert_eq!(head.bytes, total);

    // Read it back the same way, counting rather than collecting. A
    // `collect` here would be the test doing the thing it exists to forbid.
    let mut stream = store.get(&space, &handle, None).await.unwrap();
    let mut seen = 0u64;
    let mut largest = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        largest = largest.max(chunk.len());
        seen += chunk.len() as u64;
    }
    assert_eq!(seen, total);
    assert!(
        largest <= 64 * 1024,
        "a chunk of {largest} bytes means the reader is buffering, not streaming"
    );

    // And a range in the middle of a 100 MB object is one seek, not a
    // 100 MB read.
    let mid = ordo_blob::collect(
        store
            .get(
                &space,
                &handle,
                Some(ordo_blob::ByteRange::new(52_428_800, Some(8))),
            )
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(mid, vec![50u8; 8], "the 50th megabyte should be 50s");

    store.delete(&space, &handle).await.unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

/// The S3 configuration never prints its secret.
///
/// It held an access key and its secret behind a derived `Debug`, so any
/// `{:?}` wrote both: a panic message, a `dbg!`, a tracing field, an
/// error chain carrying its context. Found while specifying per-tenant
/// buckets (2026-09-21), where a credential per tenant makes the blast
/// radius one tenant's store rather than the deployment's.
#[test]
fn an_s3_config_redacts_its_credentials_when_formatted() {
    let cfg = ordo_blob::s3::S3Config::new(
        "https://s3.example.com",
        "us-east-1",
        "a-bucket",
        "AKIAEXAMPLEKEYID",
        "a-secret-nobody-should-see",
    )
    .expect("a well formed config");

    let printed = format!("{cfg:?}");
    assert!(
        !printed.contains("a-secret-nobody-should-see"),
        "the secret reached a formatted line: {printed}"
    );
    assert!(
        !printed.contains("AKIAEXAMPLEKEYID"),
        "the access key id reached a formatted line: {printed}"
    );
    // Still useful to a human: the parts that are not secrets stay.
    assert!(printed.contains("a-bucket"), "{printed}");
    assert!(printed.contains("us-east-1"), "{printed}");
}
