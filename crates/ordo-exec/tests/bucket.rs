//! B5: the BYO-bucket attachment, against real Postgres.
//!
//! What has to be true: **a repeat is a no-op that reports success, and a
//! different target is a change**. That is what lets
//! `std.space.attachBucket` be safe without the durable `ReplayLease` that
//! `PlatformExecutor` drops — for a stated reason rather than by
//! inheriting `exit_ops`' "idempotent by construction", which attaching a
//! bucket is not.

use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::custody::SpaceId;
use ordo_exec::bucket::{self, AttachOutcome, Attachment, BucketCustody};
use ordo_exec::{Pool, PoolConfig};

async fn pool(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    Fixture::load()
        .expect("fixture")
        .reset(&pg)
        .expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    bucket::ensure_table(&pool).await.expect("table");
    pool.rows("DELETE FROM \"$spacebucket\"").await.unwrap();
    pool
}

fn attachment(bucket_name: &str, custody: BucketCustody, at: i64, by: &str) -> Attachment {
    Attachment {
        endpoint: "https://s3.eu-west-1.amazonaws.com".into(),
        region: "eu-west-1".into(),
        bucket: bucket_name.into(),
        credential: "acme-prod".into(),
        custody,
        path_style: None,
        key_prefix: None,
        attached_at: at,
        attached_by: by.into(),
    }
}

#[tokio::test]
async fn a_repeat_is_unchanged_and_a_different_target_is_a_change() {
    let pool = pool("ordo_bucket_attach").await;
    let space = SpaceId("conformance".into());

    let first = attachment(
        "acme-files",
        BucketCustody::PlatformSealed,
        100,
        "mem_owner",
    );
    assert_eq!(
        bucket::attach(&pool, &space, &first).await.unwrap(),
        AttachOutcome::Attached
    );

    // A retry an hour later, by a different member of the same space. The
    // TARGET is identical, so this is the same attachment — and treating
    // it as a change would put a false "storage custody changed" in the
    // owner's stream every time a client retried.
    let retry = attachment(
        "acme-files",
        BucketCustody::PlatformSealed,
        999,
        "mem_other",
    );
    assert_eq!(
        bucket::attach(&pool, &space, &retry).await.unwrap(),
        AttachOutcome::Unchanged
    );
    let stored = bucket::lookup(&pool, &space)
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(
        stored.attached_at, 100,
        "an unchanged attach must not rewrite the row: `when` and `who` are what \
         the consent page reports, and a retry did not change custody"
    );

    // A different bucket IS a custody conversion.
    let moved = attachment(
        "acme-other",
        BucketCustody::PlatformSealed,
        200,
        "mem_owner",
    );
    assert_eq!(
        bucket::attach(&pool, &space, &moved).await.unwrap(),
        AttachOutcome::Replaced
    );
    let stored = bucket::lookup(&pool, &space)
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(stored.bucket, "acme-other");
    assert_eq!(stored.attached_at, 200);

    // So is a different custody position, with the same bucket — it
    // changes what deleting the space does to those objects, which is the
    // whole reason the choice is recorded.
    let converted = attachment("acme-other", BucketCustody::OwnPerimeter, 300, "mem_owner");
    assert_eq!(
        bucket::attach(&pool, &space, &converted).await.unwrap(),
        AttachOutcome::Replaced
    );
    assert_eq!(
        bucket::lookup(&pool, &space)
            .await
            .unwrap()
            .unwrap()
            .custody,
        BucketCustody::OwnPerimeter
    );
}

/// One bucket per space, replaced whole. A second row would make "which
/// bucket does this space use" a question with two answers.
#[tokio::test]
async fn a_space_has_one_attachment() {
    let pool = pool("ordo_bucket_one").await;
    let space = SpaceId("conformance".into());
    for (i, name) in ["a-one", "a-two", "a-three"].iter().enumerate() {
        bucket::attach(
            &pool,
            &space,
            &attachment(name, BucketCustody::PlatformSealed, i as i64, "mem_owner"),
        )
        .await
        .unwrap();
    }
    let rows = pool
        .rows("SELECT count(*) FROM \"$spacebucket\" WHERE \"$space\" = 'conformance'")
        .await
        .unwrap();
    assert_eq!(rows[0][0].as_deref(), Some("1"));
    assert_eq!(
        bucket::lookup(&pool, &space).await.unwrap().unwrap().bucket,
        "a-three"
    );
}

/// **U1's most important test.** `path_style` and `key_prefix` arrived
/// after `$spacebucket` shipped, as nullable columns with no default —
/// so a row an operator's deployment wrote months ago, naming neither
/// column, must read back with exactly the meaning it had before: derive
/// path style as `S3Config::new` does, and use today's layout with no
/// prefix.
///
/// Genuinely exercises that: the `INSERT` below names the original eight
/// columns and never mentions `path_style` or `key_prefix` at all — not
/// even as an explicit `NULL` — which is what a pre-U1 `attach` call
/// actually wrote and what `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`
/// backfills existing rows with. A row `bucket::attach` wrote itself with
/// both fields set to `None` would prove the read path handles `NULL`; it
/// would not prove the *old* write path is untouched, which is the actual
/// risk a new column carries.
#[tokio::test]
async fn an_attachment_written_before_these_columns_reads_back_unchanged() {
    let pool = pool("ordo_bucket_precolumns").await;
    let space = SpaceId("conformance".into());

    pool.rows(
        "INSERT INTO \"$spacebucket\" \
         (\"$space\", \"endpoint\", \"region\", \"bucket\", \"credential\", \"custody\", \
          \"attached_at\", \"attached_by\") \
         VALUES ('conformance', 'https://s3.eu-west-1.amazonaws.com', 'eu-west-1', \
                 'acme-files', 'acme-prod', 'platform-sealed', 100, 'mem_owner')",
    )
    .await
    .expect("a pre-U1 style insert, naming none of the new columns");

    let stored = bucket::lookup(&pool, &space)
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(stored.endpoint, "https://s3.eu-west-1.amazonaws.com");
    assert_eq!(stored.region, "eu-west-1");
    assert_eq!(stored.bucket, "acme-files");
    assert_eq!(stored.credential, "acme-prod");
    assert_eq!(stored.custody, BucketCustody::PlatformSealed);
    assert_eq!(stored.attached_at, 100);
    assert_eq!(stored.attached_by, "mem_owner");
    assert_eq!(
        stored.path_style, None,
        "a row from before this column existed must derive, not assume, a style"
    );
    assert_eq!(
        stored.key_prefix, None,
        "a row from before this column existed must use today's layout, not a stated prefix"
    );
}

/// The two new columns round-trip an explicit choice in both directions,
/// and the choice reads back distinct from the "not stated" a pre-U1 row
/// carries (proven by the test above): `Some(true)` and `Some(false)` are
/// each their own value, not folded onto `None`.
#[tokio::test]
async fn an_attachment_can_state_path_style_its_gateway_requires() {
    let pool = pool("ordo_bucket_pathstyle").await;

    let on = SpaceId("conformance".into());
    let mut want = attachment("acme-files", BucketCustody::PlatformSealed, 1, "mem_owner");
    want.path_style = Some(true);
    want.key_prefix = Some("incoming".into());
    bucket::attach(&pool, &on, &want).await.unwrap();
    let stored = bucket::lookup(&pool, &on).await.unwrap().unwrap();
    assert_eq!(stored.path_style, Some(true));
    assert_eq!(stored.key_prefix, Some("incoming".into()));

    let off = SpaceId("conformance-off".into());
    let mut want = attachment("acme-other", BucketCustody::PlatformSealed, 1, "mem_owner");
    want.path_style = Some(false);
    bucket::attach(&pool, &off, &want).await.unwrap();
    assert_eq!(
        bucket::lookup(&pool, &off)
            .await
            .unwrap()
            .unwrap()
            .path_style,
        Some(false),
        "a gateway that does NOT want path style must be able to say so explicitly"
    );
}

/// Spaces do not share attachments, and one space's attach does not reach
/// another's — the tenancy property every platform table has.
#[tokio::test]
async fn attachments_are_per_space() {
    let pool = pool("ordo_bucket_tenancy").await;
    let a = SpaceId("conformance".into());
    let b = SpaceId("other".into());
    bucket::attach(
        &pool,
        &a,
        &attachment("a-files", BucketCustody::PlatformSealed, 1, "mem_a"),
    )
    .await
    .unwrap();
    assert!(bucket::lookup(&pool, &b).await.unwrap().is_none());
    assert_eq!(
        bucket::attach(
            &pool,
            &b,
            &attachment("b-files", BucketCustody::OwnPerimeter, 1, "mem_b")
        )
        .await
        .unwrap(),
        AttachOutcome::Attached,
        "another space's attachment is not this one's"
    );
    assert_eq!(
        bucket::lookup(&pool, &a).await.unwrap().unwrap().bucket,
        "a-files"
    );
}
