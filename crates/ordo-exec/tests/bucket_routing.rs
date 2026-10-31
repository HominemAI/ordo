//! U5: `AttachedStores` reads `$spacebucket` and honours every column of
//! it, against real Postgres.
//!
//! `tests/bucket.rs` proves the storage layer: a repeat is a no-op and a
//! different target is a change. This file proves the half that reads it
//! back out: that a space with a row resolves to the bucket the row names,
//! that a space with no row resolves to the deployment's own store, and
//! that `path_style`, `key_prefix`, `bucket`, `region` and the credential
//! label all reach the wire rather than stopping at a struct field.
//!
//! # Why the end-to-end is here and not against a bucket
//!
//! `plan-tenant-storage.md` §11 U5 asks for an end-to-end that "puts one
//! tenant on a second real bucket". There is no S3 in this environment,
//! since `ordo-blob`'s own contract suite skips its S3 run unless
//! `ORDO_BLOB_S3_*` is set, and §2 B-C4 forbids `fs:` as an attachment
//! target **deliberately**, in the words of the plan, because "the obvious
//! way to make attachments testable is to let one name a directory, and
//! that is how a test double becomes a tenant's only copy". Widening
//! `S3Config::new` or teaching the resolver a second scheme to make a test
//! pass would trade the property for the test that was supposed to prove
//! it.
//!
//! So the second bucket here is a **loopback HTTP listener that records
//! what was sent to it**, and the attachment still travels the whole
//! production path to reach it: a `$spacebucket` row, a credential label,
//! `S3Config::new`, `S3Store`, SigV4, a socket. `S3Config::new` admits a
//! plaintext endpoint on loopback and only on loopback, which is the
//! self-host exception it already documents, so nothing is relaxed to
//! make this run.
//!
//! That is **stronger** than two test-constructed stores behind the
//! resolver, which was the other option. Test doubles prove a branch was
//! taken. This proves the request line: which bucket, which prefix, which
//! key. A resolver that read `key_prefix` into a field and never onto a
//! request would pass the first test and fail this one, and that bug is
//! exactly the shape a wiring increment produces.
//!
//! What is **not** proved here is a real S3 gateway's answer to those
//! requests. That stays where it already is: `ordo-blob`'s contract suite,
//! skipped unless configured, which is the one place a live bucket
//! belongs.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_blob::{BlobStore, StoreResolver};
use ordo_conformance::{Fixture, Pg};
use ordo_crypto::custody::{BlobHandle, SpaceId};
use ordo_exec::bucket::{self, AttachedStores, Attachment, BucketCustody};
use ordo_exec::creds::{BucketCredentials, Dir};
use ordo_exec::{Pool, PoolConfig};

// ── the rig ──────────────────────────────────────────────────────────────

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

fn tempdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ordo-u5-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A credential directory holding an S3 key pair under each label, in the
/// layout `creds::Dir` refuses to read any other way: a directory named by
/// the label, `0o700`, holding `access_key_id` and `secret_access_key` at
/// `0o600`.
fn credentials(labels: &[&str]) -> (PathBuf, Arc<dyn BucketCredentials>) {
    let root = tempdir("creds");
    for label in labels {
        let d = root.join(label);
        std::fs::create_dir_all(&d).unwrap();
        for (name, value) in [
            ("access_key_id", format!("AKIA{}", label.to_uppercase())),
            ("secret_access_key", format!("secret-for-{label}")),
        ] {
            let f = d.join(name);
            std::fs::write(&f, value).unwrap();
            set_mode(&f, 0o600);
        }
        set_mode(&d, 0o700);
    }
    set_mode(&root, 0o700);
    let dir = Dir::open(&root).expect("the credential directory reads");
    (root, Arc::new(dir) as Arc<dyn BucketCredentials>)
}

#[cfg(unix)]
fn set_mode(p: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_p: &std::path::Path, _mode: u32) {}

/// The deployment's own store: a filesystem double, which is what
/// `ORDO_BLOB_STORE=fs:` already is under
/// `ORDO_DEV_UNSAFE_BLOB_STORE=1`.
///
/// **This is not B-C4 being bent.** B-C4 is about an *attachment* target,
/// and no attachment here names a directory: every one travels
/// `S3Config::new`, which has no `fs:` form to name. What a deployment
/// points `ORDO_BLOB_STORE` at is its own affair and always was.
fn deployment_store() -> (PathBuf, Arc<dyn BlobStore>) {
    let dir = tempdir("default");
    let store = ordo_blob::FsStore::shared(&dir).expect("the deployment's own store");
    (dir, store)
}

fn attachment(endpoint: &str, bucket_name: &str, credential: &str) -> Attachment {
    Attachment {
        endpoint: endpoint.into(),
        region: "eu-west-1".into(),
        bucket: bucket_name.into(),
        credential: credential.into(),
        custody: BucketCustody::PlatformSealed,
        path_style: None,
        key_prefix: None,
        attached_at: 1_000,
        attached_by: "mem_owner".into(),
    }
}

async fn attach(p: &Pool, space: &str, at: &Attachment) {
    bucket::attach(p, &SpaceId(space.into()), at)
        .await
        .expect("attached");
}

/// One loopback listener standing in for a bucket's gateway: it answers
/// every request `200` with a `content-length`, and keeps the head of each
/// request it was sent, verbatim.
///
/// Verbatim rather than parsed, because the thing under test is what the
/// attachment's columns did to the request, and a parser here would be a
/// second chance to normalise away the difference.
///
/// # Why the endpoint is `localhost` and not `127.0.0.1`
///
/// Virtual-host style is a **real** addressing mode in this client, not a
/// header: `S3Store::call` connects to `<bucket>.<host>` rather than to
/// `<host>`, which is correct and is what makes B-C2's failure mode ("a
/// DNS lookup for `<bucket>.<host>` that does not resolve") the thing it
/// says it is. `<bucket>.127.0.0.1` is not a name and resolves nowhere,
/// so a gateway on an IP literal could only ever be reached path style
/// and the override would be untestable in the one direction that
/// matters. Everything under `.localhost` resolves to loopback (RFC
/// 6761), so the listener binds whatever family `localhost` itself
/// resolves to and both addressing modes arrive at it.
struct Gateway {
    endpoint: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Gateway {
    fn start(calls: usize) -> Gateway {
        use std::net::ToSocketAddrs;
        let addr = "localhost:0"
            .to_socket_addrs()
            .expect("localhost resolves")
            .next()
            .expect("localhost resolves to something");
        let listener = TcpListener::bind(addr).expect("a loopback port");
        let endpoint = format!("http://localhost:{}", listener.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        std::thread::spawn(move || {
            for _ in 0..calls {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {
                            if line == "\r\n" {
                                break;
                            }
                            head.push_str(&line);
                        }
                        Err(_) => break,
                    }
                }
                recorder.lock().expect("recorder").push(head);
                let mut stream = reader.into_inner();
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 4096\r\nconnection: close\r\n\r\n",
                );
                let _ = stream.flush();
            }
        });
        Gateway { endpoint, seen }
    }

    /// The request line of the `n`th request, which is
    /// `<METHOD> <path> HTTP/1.1`.
    fn request_line(&self, n: usize) -> String {
        let seen = self.seen.lock().expect("recorder");
        seen.get(n)
            .unwrap_or_else(|| panic!("no request {n} arrived; {} did", seen.len()))
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn count(&self) -> usize {
        self.seen.lock().expect("recorder").len()
    }
}

fn handle() -> BlobHandle {
    let mut h = BlobHandle::default();
    for (i, b) in h.iter_mut().enumerate() {
        *b = i as u8;
    }
    h
}

/// The hex spelling of [`handle`], which is what an object key ends with.
fn handle_hex() -> String {
    ordo_crypto::hex(&handle())
}

// ── the switch changes nothing until something is attached ───────────────

/// **A space with no row resolves to the deployment's own store**, and it
/// is the identical `Arc`, not an equivalent one.
///
/// This is the property that makes `ORDO_BLOB_ROUTING=attached` safe to
/// turn on ahead of the attach op, and it is asserted on pointer identity
/// because that is the strongest available statement: there is not a
/// second store standing in for the default, there is *the* default.
#[tokio::test]
async fn a_space_with_no_attachment_resolves_to_the_deployments_own_store() {
    let p = pool("ordo_u5_no_row").await;
    let (creds_root, creds) = credentials(&["acme-prod"]);
    let (store_dir, default) = deployment_store();

    let stores = AttachedStores::new(p.clone(), Arc::clone(&default), creds);
    for space in ["tenant-a", "tenant-b"] {
        let got = stores
            .resolve(&SpaceId(space.into()))
            .await
            .expect("a space with no row resolves");
        assert!(
            Arc::ptr_eq(&got, &default),
            "{space} did not resolve to the deployment's own store"
        );
    }
    assert_eq!(
        stores.stores_built(),
        0,
        "a deployment that has attached nothing built a store it did not need"
    );

    std::fs::remove_dir_all(&creds_root).ok();
    std::fs::remove_dir_all(&store_dir).ok();
}

// ── the row is read, and it decides ──────────────────────────────────────

/// **An attached space resolves to its own bucket, and its neighbour on
/// the default is untouched.**
///
/// The two halves are one test on purpose. "The attached tenant reaches
/// its bucket" and "the tenant on the default still reaches the
/// deployment's store" are the two ways a resolver goes wrong, and they go
/// wrong in opposite directions: one sends a tenant's bytes to a store
/// nobody chose, the other records a custody change and keeps writing to
/// the deployment's own bucket. Proving them apart would leave either one
/// free to be true only when the other is not exercised.
#[tokio::test]
async fn an_attached_space_reaches_its_own_bucket_and_its_neighbour_does_not() {
    let p = pool("ordo_u5_attached").await;
    let (creds_root, creds) = credentials(&["acme-prod"]);
    let (store_dir, default) = deployment_store();
    let gateway = Gateway::start(1);

    attach(
        &p,
        "tenant-a",
        &attachment(&gateway.endpoint, "acme-files", "acme-prod"),
    )
    .await;

    let stores = AttachedStores::new(p.clone(), Arc::clone(&default), creds);

    let attached = stores
        .resolve(&SpaceId("tenant-a".into()))
        .await
        .expect("the attached space resolves");
    assert!(
        !Arc::ptr_eq(&attached, &default),
        "an attached space resolved to the deployment's own store, which is \
         the exact failure the attachment exists to rule out"
    );
    assert_eq!(
        attached.binding(),
        "s3",
        "an attachment is an S3 target and nothing else; see §2 B-C4"
    );

    let neighbour = stores
        .resolve(&SpaceId("tenant-b".into()))
        .await
        .expect("the unattached space resolves");
    assert!(
        Arc::ptr_eq(&neighbour, &default),
        "attaching one tenant moved another tenant's bytes"
    );

    // And the store the attached space got is really pointed at the
    // attachment's bucket: it talks to the endpoint the row named, about
    // the bucket the row named.
    attached
        .head(&SpaceId("tenant-a".into()), &handle())
        .await
        .expect("the gateway answered");
    assert_eq!(gateway.count(), 1, "the attached store dialled once");
    let line = gateway.request_line(0);
    assert!(
        line.starts_with("HEAD /acme-files/tenant-a/"),
        "the request did not address the attachment's bucket: {line}"
    );

    std::fs::remove_dir_all(&creds_root).ok();
    std::fs::remove_dir_all(&store_dir).ok();
}

/// **`path_style` and `key_prefix` reach the request line**, which is the
/// only place they can be said to have been honoured.
///
/// Both are U1 columns (§2 B-C2) whose whole purpose is to change where in
/// a bucket an object is addressed, so a resolver that read them into a
/// config and never onto a request would be a resolver that talks to the
/// right bucket about the wrong objects, and would look completely
/// healthy doing it until a tenant reported missing files.
///
/// The two directions are both asserted because `S3Config::new` *derives*
/// path style, and derives it **on** for a loopback endpoint. A test that
/// only set `Some(true)` would agree with the derivation and prove
/// nothing. `Some(false)` against loopback is the case where the row and
/// the derivation disagree, and the row has to win, which is precisely
/// what B-C2 asks for: "a self-hosted MinIO or Ceph behind TLS on a real
/// hostname needs path style, the derivation gives it virtual-host style".
#[tokio::test]
async fn an_attachments_path_style_and_key_prefix_reach_the_request() {
    let p = pool("ordo_u5_columns").await;
    let (creds_root, creds) = credentials(&["acme-prod"]);
    let (store_dir, default) = deployment_store();
    let gateway = Gateway::start(2);

    // Path style stated, with a prefix: `/<bucket>/<prefix>/<space>/<hex>`.
    let mut want = attachment(&gateway.endpoint, "acme-files", "acme-prod");
    want.path_style = Some(true);
    want.key_prefix = Some("ordo_objects".into());
    attach(&p, "tenant-a", &want).await;

    // Virtual-host style stated **against loopback**, where the derivation
    // would have said path style: `/<prefix>/<space>/<hex>`, no bucket
    // segment.
    let mut other = attachment(&gateway.endpoint, "globex-files", "acme-prod");
    other.path_style = Some(false);
    other.key_prefix = Some("under_here".into());
    attach(&p, "tenant-b", &other).await;

    let stores = AttachedStores::new(p.clone(), default, creds);
    let hex = handle_hex();

    stores
        .resolve(&SpaceId("tenant-a".into()))
        .await
        .expect("a")
        .head(&SpaceId("tenant-a".into()), &handle())
        .await
        .expect("the gateway answered");
    assert_eq!(
        gateway.request_line(0),
        format!("HEAD /acme-files/ordo_objects/tenant-a/{hex} HTTP/1.1"),
        "path style or the key prefix did not reach the request"
    );

    // **Asserted without a socket, deliberately.** Virtual-host style
    // addresses `<bucket>.<host>`, and `globex-files.localhost` resolves
    // on macOS, which maps every `*.localhost`, and not under glibc,
    // which does not. Dialling here passed on a developer machine and
    // failed on CI with `Name or service not known`, so this reads the
    // request target the store would use instead. The socket above
    // already proves a request goes out; what is under test here is
    // which target it names.
    let mut cfg = ordo_blob::S3Config::new(
        &gateway.endpoint,
        "us-east-1",
        "globex-files",
        "id",
        "secret",
    )
    .expect("config");
    cfg.path_style = false;
    cfg.key_prefix = Some("under_here".into());
    let target = ordo_blob::S3Store::new(cfg)
        .request_target(&SpaceId("tenant-b".into()), &handle())
        .expect("a request target");
    assert_eq!(
        target,
        format!("/under_here/tenant-b/{hex}"),
        "`path_style: Some(false)` did not override the loopback derivation"
    );

    std::fs::remove_dir_all(&creds_root).ok();
    std::fs::remove_dir_all(&store_dir).ok();
}

/// **The space segment survives the prefix.**
///
/// The crate doc of `ordo-blob` rests a stated property on it: an operator
/// holding a bucket and an incident can answer "which objects are this
/// tenant's" from a listing, without the database that is currently down.
/// A prefix that *replaced* the space segment rather than preceding it
/// would take that away, and would do it silently, so the ordering is
/// asserted rather than read off the code.
#[tokio::test]
async fn the_key_prefix_goes_ahead_of_the_space_and_never_instead_of_it() {
    let p = pool("ordo_u5_prefix_order").await;
    let (creds_root, creds) = credentials(&["acme-prod"]);
    let (store_dir, default) = deployment_store();
    let gateway = Gateway::start(1);

    let mut want = attachment(&gateway.endpoint, "acme-files", "acme-prod");
    want.key_prefix = Some("tenant_data".into());
    attach(&p, "tenant-a", &want).await;

    let stores = AttachedStores::new(p.clone(), default, creds);
    stores
        .resolve(&SpaceId("tenant-a".into()))
        .await
        .expect("resolved")
        .head(&SpaceId("tenant-a".into()), &handle())
        .await
        .expect("the gateway answered");

    let line = gateway.request_line(0);
    let prefix_at = line.find("tenant_data").expect("the prefix is in the key");
    let space_at = line.find("/tenant-a/").expect("the space is in the key");
    assert!(
        prefix_at < space_at,
        "the prefix must precede the space segment, not replace or follow it: {line}"
    );

    std::fs::remove_dir_all(&creds_root).ok();
    std::fs::remove_dir_all(&store_dir).ok();
}

// ── the credential, and the two refusals ─────────────────────────────────

/// **A label this node does not hold refuses, and no store is built.**
///
/// The row names a credential and never carries one (this module's own
/// rule), so a node that has not been given the material cannot sign for
/// that bucket. Refusing here is the same posture `route::target_config`
/// takes on the database side, never a dial as nobody, and it matters
/// more here, because `S3Config::new` would refuse an empty key anyway but
/// only after a store had been constructed and cached.
///
/// The refusal also names the label and never anything under it, which is
/// what lets it be logged.
#[tokio::test]
async fn a_credential_label_this_node_does_not_hold_refuses_before_a_store_exists() {
    let p = pool("ordo_u5_no_label").await;
    let (creds_root, creds) = credentials(&["acme-prod"]);
    let (store_dir, default) = deployment_store();

    attach(
        &p,
        "tenant-a",
        &attachment(
            "https://s3.eu-west-1.amazonaws.com",
            "globex-files",
            "globex-prod",
        ),
    )
    .await;

    let stores = AttachedStores::new(p.clone(), default, creds);
    let Err(err) = stores.resolve(&SpaceId("tenant-a".into())).await else {
        panic!("a label this node does not hold must refuse");
    };
    let text = err.to_string();
    assert!(
        text.contains("globex-prod"),
        "the refusal does not name the label an operator would install: {text}"
    );
    assert!(
        !text.contains("secret-for-") && !text.contains("AKIA"),
        "the refusal carries material: {text}"
    );
    assert_eq!(
        stores.stores_built(),
        0,
        "a store was built for a target this node cannot sign to"
    );

    std::fs::remove_dir_all(&creds_root).ok();
    std::fs::remove_dir_all(&store_dir).ok();
}

/// **A hostile key prefix in the row is refused rather than sanitised**,
/// even though the door already refuses one.
///
/// The door's check (`BucketOps::attach`, U1) is the one a caller meets.
/// This one is the one a *row* meets, and the row is what the resolver
/// actually reads: it can predate the door's check, it can be written by a
/// future caller, and it can be edited with `psql`. Sanitising would be
/// worse than either, for `space_prefix`'s own reason: two different
/// prefixes can collapse onto one location, and this prefix is what
/// separates the platform's objects from the rest of the tenant's bucket.
#[tokio::test]
async fn a_hostile_key_prefix_in_the_row_is_refused_rather_than_sanitised() {
    let p = pool("ordo_u5_hostile_prefix").await;
    let (creds_root, creds) = credentials(&["acme-prod"]);
    let (store_dir, default) = deployment_store();

    for hostile in ["../../etc", "a/b", "with space", "%2e%2e"] {
        p.rows("DELETE FROM \"$spacebucket\"").await.unwrap();
        let mut want = attachment(
            "https://s3.eu-west-1.amazonaws.com",
            "acme-files",
            "acme-prod",
        );
        want.key_prefix = Some(hostile.to_string());
        attach(&p, "tenant-a", &want).await;

        let stores = AttachedStores::new(p.clone(), Arc::clone(&default), Arc::clone(&creds));
        let Err(err) = stores.resolve(&SpaceId("tenant-a".into())).await else {
            panic!("a hostile prefix must refuse: {hostile}");
        };
        assert!(
            err.to_string().contains("key prefix"),
            "the refusal does not say what was wrong: {err}"
        );
    }

    std::fs::remove_dir_all(&creds_root).ok();
    std::fs::remove_dir_all(&store_dir).ok();
}

// ── the cache ────────────────────────────────────────────────────────────

/// **Stores are cached by target identity, not by space.**
///
/// The sizing argument `plan-tenant-storage.md` rests on is that every
/// per-bucket cost scales with the number of distinct *targets*, which is
/// only true if a hundred spaces in one bucket share one store and one
/// signer. Asserted on a count of stores **built** rather than stores
/// held, because a count of what is held cannot tell a reuse from a
/// rebuild that replaced its predecessor.
#[tokio::test]
async fn spaces_sharing_a_target_share_one_store_and_a_different_target_gets_its_own() {
    let p = pool("ordo_u5_identity_cache").await;
    let (creds_root, creds) = credentials(&["acme-prod", "globex-prod"]);
    let (store_dir, default) = deployment_store();

    let one = attachment(
        "https://s3.eu-west-1.amazonaws.com",
        "acme-files",
        "acme-prod",
    );
    attach(&p, "tenant-a", &one).await;
    // The same target, restated by a different owner at a different time:
    // `identity` excludes both, so this is one target and not two.
    let mut same = one.clone();
    same.attached_at = 9_999;
    same.attached_by = "mem_someone_else".into();
    attach(&p, "tenant-b", &same).await;
    attach(
        &p,
        "tenant-c",
        &attachment(
            "https://s3.us-east-1.amazonaws.com",
            "globex-files",
            "globex-prod",
        ),
    )
    .await;

    let stores = AttachedStores::new(p.clone(), default, creds);
    let a = stores.resolve(&SpaceId("tenant-a".into())).await.unwrap();
    let b = stores.resolve(&SpaceId("tenant-b".into())).await.unwrap();
    let c = stores.resolve(&SpaceId("tenant-c".into())).await.unwrap();

    assert!(
        Arc::ptr_eq(&a, &b),
        "two spaces in one bucket got two stores and two signers"
    );
    assert!(
        !Arc::ptr_eq(&a, &c),
        "two different buckets shared one store"
    );
    assert_eq!(
        stores.stores_built(),
        2,
        "three attached spaces over two targets should build two stores"
    );

    // Resolving again builds nothing: this is the cache, not a coincidence
    // of ordering.
    stores.resolve(&SpaceId("tenant-a".into())).await.unwrap();
    stores.resolve(&SpaceId("tenant-c".into())).await.unwrap();
    assert_eq!(stores.stores_built(), 2, "a repeat resolve rebuilt a store");

    std::fs::remove_dir_all(&creds_root).ok();
    std::fs::remove_dir_all(&store_dir).ok();
}

/// **A cached answer expires, and the node that attached can drop its own
/// at once.**
///
/// The interval is what bounds the damage when a space is attached, or
/// re-attached, after a node has already answered for it. `AttachedPools`
/// caches only its *misses*, because `spacedb::attach` refuses to move an
/// attached space. `bucket::attach` has no such refusal, because
/// `AttachOutcome::Replaced` is a documented custody conversion, so a hit
/// cached for the life of the process would leave a node writing into a
/// bucket the owner has detached. Both arms expire here, and this is the
/// test that says so.
#[tokio::test]
async fn an_answer_expires_and_the_attaching_node_can_drop_its_own_at_once() {
    let p = pool("ordo_u5_ttl").await;
    let (creds_root, creds) = credentials(&["acme-prod"]);
    let (store_dir, default) = deployment_store();

    let stores = AttachedStores::new(p.clone(), Arc::clone(&default), creds)
        .with_ttl(Duration::from_millis(150));

    // Nothing attached: the default, and the answer is remembered.
    let space = SpaceId("tenant-a".into());
    assert!(Arc::ptr_eq(
        &stores.resolve(&space).await.unwrap(),
        &default
    ));

    attach(
        &p,
        "tenant-a",
        &attachment(
            "https://s3.eu-west-1.amazonaws.com",
            "acme-files",
            "acme-prod",
        ),
    )
    .await;

    // Still the default, because the miss is still remembered. This is the
    // window the interval bounds, asserted rather than left implicit: a
    // reader who believes attaching takes effect instantly on every node
    // is reading something this test says is not true.
    assert!(
        Arc::ptr_eq(&stores.resolve(&space).await.unwrap(), &default),
        "the cached answer was not used at all, so the interval below proves nothing"
    );

    // The node that performed the attach drops its own answer.
    stores.forget(&space);
    assert!(
        !Arc::ptr_eq(&stores.resolve(&space).await.unwrap(), &default),
        "`forget` did not make the next resolve read the row"
    );

    // And a node that did not attach gets there on its own, by expiry.
    let other = AttachedStores::new(p.clone(), Arc::clone(&default), {
        let (_root, c) = credentials(&["acme-prod"]);
        c
    })
    .with_ttl(Duration::from_millis(150));
    assert!(!Arc::ptr_eq(
        &other.resolve(&space).await.unwrap(),
        &default
    ));
    p.rows("DELETE FROM \"$spacebucket\"").await.unwrap();
    assert!(
        !Arc::ptr_eq(&other.resolve(&space).await.unwrap(), &default),
        "the hit was not cached at all"
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        Arc::ptr_eq(&other.resolve(&space).await.unwrap(), &default),
        "a cached hit outlived the interval, so a detached tenant would keep \
         being written to the bucket it left"
    );

    std::fs::remove_dir_all(&creds_root).ok();
    std::fs::remove_dir_all(&store_dir).ok();
}
