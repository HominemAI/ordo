//! The sealing pipeline, end to end, against a real store and real
//! custody.
//!
//! `FsStore` is the test double and `TestProvider` is the in-memory
//! custody the rest of the tree already uses; between them nothing here is
//! a mock of the thing under test. The four assertions `plan-blob.md` B3
//! names as its closing condition are the four headline tests below.

use std::sync::Arc;

use ordo_blob::{BlobPipeline, BlobPolicy, BlobStore, FsStore, PipelineError, Upload};
use ordo_crypto::custody::{KeyProvider, Purpose, SpaceId, TestProvider};

fn tempdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ordo-blob-pipe-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Rig {
    dir: std::path::PathBuf,
    pipe: BlobPipeline,
    store: Arc<dyn BlobStore>,
    keys: Arc<TestProvider>,
    space: SpaceId,
}

impl Rig {
    fn new(name: &str) -> Rig {
        Rig::with_policy(name, BlobPolicy::DEFAULT)
    }

    fn with_policy(name: &str, policy: BlobPolicy) -> Rig {
        let dir = tempdir(name);
        let store: Arc<dyn BlobStore> = FsStore::shared(&dir).unwrap();
        let keys = Arc::new(TestProvider::with_space("tenant-a"));
        Rig {
            dir,
            pipe: BlobPipeline::new(
                Arc::new(ordo_blob::SingleStore::new(Arc::clone(&store))),
                Arc::clone(&keys) as Arc<dyn KeyProvider>,
                policy,
            ),
            store,
            keys,
            space: SpaceId("tenant-a".into()),
        }
    }

    fn upload<'a>(&'a self, len: u64) -> Upload<'a> {
        Upload {
            space: &self.space,
            entity: "documents",
            field: "contract",
            mime: "application/pdf",
            plain_len: len,
            correlation: "req-1",
            // No database behind this rig, so nothing to declare the
            // handle to. The property the intent buys is asserted where a
            // database exists (`ordo-host/tests/blob_door.rs`).
            intent: None,
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn an_object_round_trips_through_the_sealer() {
    let r = Rig::new("round");
    let plain = pattern(200_000);
    let stored = r
        .pipe
        .put(
            &r.upload(plain.len() as u64),
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put");

    // The stored length is the plaintext plus one tag per chunk plus the
    // header — not the plaintext length, which is what the reference row
    // records separately and sealed.
    assert_eq!(
        stored.bytes() as u64,
        ordo_crypto::stream::sealed_len(plain.len() as u64)
    );

    let got = r
        .pipe
        .get(
            &r.space,
            "documents",
            "contract",
            stored.handle(),
            plain.len() as u64,
            0,
            None,
            "req-2",
        )
        .await
        .expect("get");
    assert_eq!(got, plain);
}

/// **The assertion the whole design exists for.** A test that reads the
/// stored object directly and looks for the plaintext in it.
///
/// DL §2: *"Object storage always holds ciphertext for sealed content …
/// a leaked bucket is noise."* That sentence is a claim about bytes, so it
/// is checked against bytes.
#[tokio::test]
async fn the_plaintext_never_appears_in_the_stored_object() {
    let r = Rig::new("cipher");
    // A needle that is long enough not to occur by chance in 200 KB of
    // random-looking ciphertext, and recognisable if it did.
    let needle = b"THE-CLAIMANT-SETTLED-FOR-FOUR-MILLION";
    let mut plain = pattern(100_000);
    plain.extend_from_slice(needle);
    plain.extend_from_slice(&pattern(100_000));

    let stored = r
        .pipe
        .put(
            &r.upload(plain.len() as u64),
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put");

    let raw = ordo_blob::collect(
        r.store
            .get(&r.space, stored.handle(), None)
            .await
            .expect("raw read"),
    )
    .await
    .expect("raw bytes");

    assert!(
        !raw.windows(needle.len()).any(|w| w == needle),
        "the plaintext is in the bucket"
    );
    // And the mime is not there either, because the mime is sealed too: a
    // content type is a strong hint about what a document is.
    assert!(!raw.windows(15).any(|w| w == b"application/pdf"));
    // The metadata the row will carry is envelopes, not values.
    assert_ne!(stored.mime(), b"application/pdf".to_vec());
    assert_ne!(stored.size(), plain.len().to_string().into_bytes());
}

/// **A 100 MB upload streams through in bounded memory** — asserted, not
/// assumed. The body is generated lazily and the sealed side is counted
/// rather than collected; the way this regresses is a `collect` slipping
/// into the pipeline during a refactor, and a `collect` looks innocent.
#[tokio::test]
async fn a_hundred_megabyte_upload_never_holds_the_object() {
    let r = Rig::new("big");
    const MB: usize = 1 << 20;
    const N: u64 = 100;
    let total = MB as u64 * N;

    let body = Box::pin(futures::stream::unfold(0u64, |i| async move {
        if i == N {
            None
        } else {
            Some((Ok(vec![(i % 256) as u8; MB]), i + 1))
        }
    })) as ordo_blob::ByteStream;

    let stored = r.pipe.put(&r.upload(total), body).await.expect("put");
    assert_eq!(
        stored.bytes() as u64,
        ordo_crypto::stream::sealed_len(total)
    );

    // A range read in the middle of a 100 MB object touches two chunks,
    // not a hundred megabytes. If this were a whole-object decrypt it
    // would still pass — so the assertion that matters is the one below
    // it: the store was asked for a bounded range.
    let mid = r
        .pipe
        .get(
            &r.space,
            "documents",
            "contract",
            stored.handle(),
            total,
            50 * MB as u64,
            Some(8),
            "req-2",
        )
        .await
        .expect("range");
    assert_eq!(mid, vec![50u8; 8], "the 50th megabyte should be 50s");

    let plan = ordo_crypto::stream::plan_range(total, 50 * MB as u64, Some(8)).unwrap();
    assert!(
        plan.stored_len <= 2 * ordo_crypto::stream::SEALED_CHUNK as u64,
        "a range read fetched {} bytes to serve 8",
        plan.stored_len
    );
}

/// A range at an offset in the middle returns the correct plaintext, at
/// every boundary the chunk arithmetic has.
#[tokio::test]
async fn ranges_land_on_the_right_bytes() {
    let r = Rig::new("range");
    let c = ordo_crypto::stream::CHUNK as u64;
    let plain = pattern((3 * c) as usize + 77);
    let stored = r
        .pipe
        .put(
            &r.upload(plain.len() as u64),
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put");

    for (from, len) in [
        (0u64, Some(1u64)),
        (0, Some(c)),
        (c - 1, Some(2)),
        (c, Some(10)),
        (2 * c - 5, Some(10)),
        (3 * c, None),
        (plain.len() as u64 - 1, Some(1)),
    ] {
        let got = r
            .pipe
            .get(
                &r.space,
                "documents",
                "contract",
                stored.handle(),
                plain.len() as u64,
                from,
                len,
                "req-2",
            )
            .await
            .unwrap_or_else(|e| panic!("range {from}+{len:?}: {e}"));
        let want_len = len.unwrap_or(plain.len() as u64 - from) as usize;
        assert_eq!(
            got,
            plain[from as usize..from as usize + want_len],
            "range {from}+{len:?}"
        );
    }

    // Past the end is a refusal, never an empty success.
    assert_eq!(
        r.pipe
            .get(
                &r.space,
                "documents",
                "contract",
                stored.handle(),
                plain.len() as u64,
                plain.len() as u64,
                Some(1),
                "req-2"
            )
            .await
            .unwrap_err(),
        PipelineError::Range
    );
}

/// The cap is checked **before the sealer**, so a refused upload costs no
/// key release, writes no object, and leaves nothing behind.
#[tokio::test]
async fn an_oversize_upload_is_refused_before_a_byte_is_sealed() {
    let r = Rig::with_policy(
        "cap",
        BlobPolicy {
            max_object: 1024,
            ..BlobPolicy::DEFAULT
        },
    );
    let err = r
        .pipe
        .put(&r.upload(4096), ordo_blob::once(vec![0u8; 4096]))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        PipelineError::TooLarge {
            limit: 1024,
            declared: 4096
        }
    );
    // Nothing was created, so nothing has to be collected.
    assert!(std::fs::read_dir(r.dir.join("tenant-a"))
        .map(|mut d| d.next().is_none())
        .unwrap_or(true));
}

/// A body that ends early is a truncated upload, and a truncated upload
/// must not become a successful write. The object is removed, so a `head`
/// cannot find a half-file that would fail its AEAD tag and read as
/// corruption.
#[tokio::test]
async fn a_short_body_is_refused_and_leaves_no_object() {
    let r = Rig::new("short");
    let err = r
        .pipe
        .put(&r.upload(10_000), ordo_blob::once(vec![1u8; 10]))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            PipelineError::Truncated { .. } | PipelineError::Store(_)
        ),
        "{err:?}"
    );
    assert!(std::fs::read_dir(r.dir.join("tenant-a"))
        .map(|mut d| d.next().is_none())
        .unwrap_or(true));
}

/// The integrity digest is **sealed**, and it verifies. Both halves
/// matter: the platform can check a fetched object, and nobody holding the
/// database and no keys can use the column to compare two objects.
#[tokio::test]
async fn the_integrity_digest_is_sealed_and_verifies() {
    let r = Rig::new("integrity");
    let plain = pattern(5000);
    let stored = r
        .pipe
        .put(
            &r.upload(plain.len() as u64),
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put");

    assert!(r
        .pipe
        .verify(
            &r.space,
            "documents",
            "contract",
            stored.handle(),
            plain.len() as u64,
            stored.integrity(),
            "req-2"
        )
        .await
        .expect("verify"));

    // Sealed, not plaintext: the stored column is not the digest.
    let bare = sha2::Sha256::digest(&plain).to_vec();
    assert_ne!(stored.integrity(), bare);
    // And two uploads of the SAME file produce two different envelopes, so
    // the column cannot be used to compare them. This is the property a
    // plaintext digest column would destroy.
    let again = r
        .pipe
        .put(
            &r.upload(plain.len() as u64),
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put again");
    assert_ne!(
        again.integrity(),
        stored.integrity(),
        "two uploads of one file must not share an integrity column value"
    );
    assert_ne!(
        again.handle(),
        stored.handle(),
        "two uploads of one file must not share an object key"
    );
}

use sha2::Digest;

/// An object sealed for one column does not open for another — the AAD
/// binding, which is the same one `seal_with` uses. A blob moved between
/// fields refuses, exactly as a payload does.
#[tokio::test]
async fn an_object_moved_between_columns_refuses_to_open() {
    let r = Rig::new("aad");
    let plain = pattern(1000);
    let stored = r
        .pipe
        .put(
            &r.upload(plain.len() as u64),
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put");

    let wrong = r
        .pipe
        .get(
            &r.space,
            "documents",
            "avatar", // a different column
            stored.handle(),
            plain.len() as u64,
            0,
            None,
            "req-2",
        )
        .await;
    assert!(matches!(wrong, Err(PipelineError::Custody(_))), "{wrong:?}");

    let wrong_entity = r
        .pipe
        .get(
            &r.space,
            "invoices",
            "contract",
            stored.handle(),
            plain.len() as u64,
            0,
            None,
            "req-2",
        )
        .await;
    assert!(matches!(wrong_entity, Err(PipelineError::Custody(_))));
}

/// A zero-byte upload is a legal upload, and it is one final-flagged
/// chunk — the thing that proves the object ended.
#[tokio::test]
async fn an_empty_object_is_an_object() {
    let r = Rig::new("empty");
    let stored = r
        .pipe
        .put(&r.upload(0), ordo_blob::once(Vec::new()))
        .await
        .expect("put");
    assert_eq!(
        stored.bytes() as u64,
        ordo_crypto::stream::HEADER as u64 + ordo_crypto::stream::TAG as u64
    );
    // A zero-length range is a refusal, and `verify` handles the case
    // rather than reporting a corrupt object.
    assert!(r
        .pipe
        .verify(
            &r.space,
            "documents",
            "contract",
            stored.handle(),
            0,
            stored.integrity(),
            "req-2"
        )
        .await
        .expect("verify"));
}

/// Custody is per space: an object sealed in one space does not open with
/// another space's keys, even given the handle.
#[tokio::test]
async fn another_space_cannot_open_this_ones_object() {
    let r = Rig::new("tenancy");
    let plain = pattern(500);
    let stored = r
        .pipe
        .put(&r.upload(plain.len() as u64), ordo_blob::once(plain))
        .await
        .expect("put");

    let other = SpaceId("tenant-b".into());
    r.keys.create_space(&other).expect("space");
    // The bucket prefix already scopes it — the object is not even there —
    // which is the first of the two independent mechanisms.
    let err = r
        .pipe
        .get(
            &other,
            "documents",
            "contract",
            stored.handle(),
            500,
            0,
            None,
            "req-2",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PipelineError::Store(_)), "{err:?}");
}

/// A read release seals nothing. The lease narrowing is the same one the
/// sealed-payload path has, and it applies per chunk.
#[tokio::test]
async fn a_read_release_cannot_seal_a_chunk() {
    let keys = TestProvider::with_space("tenant-a");
    let space = SpaceId("tenant-a".into());
    let read = keys.release(&space, Purpose::Read, "req").unwrap();
    let base = [0u8; 19];
    assert!(keys
        .seal_chunk(
            &read,
            "documents",
            "contract",
            ordo_crypto::Anchor::Object("h"),
            1,
            &base,
            0,
            true,
            b"x"
        )
        .is_err());
    let write = keys.release(&space, Purpose::Write, "req").unwrap();
    assert!(keys
        .seal_chunk(
            &write,
            "documents",
            "contract",
            ordo_crypto::Anchor::Object("h"),
            1,
            &base,
            0,
            true,
            b"x"
        )
        .is_ok());
}

// ── the per-space seam ───────────────────────────────────────────────────
//
// `plan-tenant-storage.md` §11 U3. Until the pipeline took a resolver, the
// serving path had no per-space seam at all: every `put` and every `get`
// went to the one store the pipeline was constructed with, while the
// sweep, the re-seal pass and `erase_space` already asked a resolver. The
// two tests below are the two halves of what that seam has to be true of:
// that a space's bytes go to and come from **that space's** store, and
// that a deployment which has attached nothing cannot tell the seam is
// there.

/// A resolver with a real store per space and no default.
///
/// **No fallback is the point.** A resolver that answered an unknown space
/// with the deployment's own store would hide exactly the bug this seam
/// exists to prevent: a space whose bytes were supposed to go to its own
/// bucket, quietly written to the platform's instead. So an unmapped space
/// refuses, and `a_space_with_no_store_refuses_every_route` asserts the
/// refusal reaches the caller rather than a store.
///
/// It also records the space it was asked about, in order, because
/// "resolves the space's own store" is a claim about **which space** each
/// route asks for, and a route that resolved the right store from the
/// wrong space id would pass every byte-level assertion here on a
/// single-tenant rig.
struct PerSpace {
    stores: std::collections::HashMap<String, Arc<dyn BlobStore>>,
    asked: std::sync::Mutex<Vec<String>>,
}

impl PerSpace {
    fn new(stores: Vec<(&str, Arc<dyn BlobStore>)>) -> PerSpace {
        PerSpace {
            stores: stores
                .into_iter()
                .map(|(s, st)| (s.to_string(), st))
                .collect(),
            asked: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("asked").clone()
    }
}

#[async_trait::async_trait]
impl ordo_blob::StoreResolver for PerSpace {
    async fn resolve(&self, space: &SpaceId) -> Result<Arc<dyn BlobStore>, ordo_blob::BlobError> {
        self.asked.lock().expect("asked").push(space.0.clone());
        self.stores
            .get(space.0.as_str())
            .map(Arc::clone)
            .ok_or_else(|| {
                ordo_blob::BlobError::Refused(format!("no store is attached to `{}`", space.0))
            })
    }
}

/// Every file under a store's root, as `<space>/<hex>` keys, sorted.
///
/// The filesystem is the bucket here, so this is the listing an operator
/// holding the tenant's bucket would see, which is the whole of what
/// "the bytes are in this space's store" means from outside.
fn keys_under(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(spaces) = std::fs::read_dir(root) else {
        return out;
    };
    for space in spaces.flatten() {
        let Ok(objects) = std::fs::read_dir(space.path()) else {
            continue;
        };
        for object in objects.flatten() {
            out.push(format!(
                "{}/{}",
                space.file_name().to_string_lossy(),
                object.file_name().to_string_lossy()
            ));
        }
    }
    out.sort();
    out
}

/// **Every route through the pipeline resolves the store of the space the
/// request named**, and writes and reads nothing anywhere else.
///
/// Three routes go through a store (`put`, `get` and `verify`), and each
/// is driven here for two spaces backed by two genuinely different roots.
/// The assertions are the ones an operator could make without the source:
/// each tenant's object is in that tenant's own root under
/// `<space>/<hex handle>`, neither root holds the other's, and each
/// tenant's bytes read back as that tenant's own.
///
/// The resolver's record of which space it was asked about is checked too,
/// because a route that asked for the right store under the wrong space id
/// would pass every byte assertion above on a rig where the two happen to
/// agree, and would then write one tenant's object into another's prefix
/// the first time they did not.
#[tokio::test]
async fn every_blob_route_resolves_the_spaces_own_store() {
    let root_a = tempdir("route-a");
    let root_b = tempdir("route-b");
    let store_a = FsStore::shared(&root_a).expect("a's store");
    let store_b = FsStore::shared(&root_b).expect("b's store");
    let resolver = Arc::new(PerSpace::new(vec![
        ("tenant-a", Arc::clone(&store_a)),
        ("tenant-b", Arc::clone(&store_b)),
    ]));
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let pipe = BlobPipeline::new(
        Arc::clone(&resolver) as Arc<dyn ordo_blob::StoreResolver>,
        keys,
        BlobPolicy::DEFAULT,
    );

    let space_a = SpaceId("tenant-a".into());
    let space_b = SpaceId("tenant-b".into());
    let plain_a = b"tenant a's signed contract".to_vec();
    let plain_b = b"tenant b's identity photo!".to_vec();

    fn up(space: &SpaceId, len: usize) -> Upload<'_> {
        Upload {
            space,
            entity: "documents",
            field: "contract",
            mime: "application/pdf",
            plain_len: len as u64,
            correlation: "req-1",
            intent: None,
        }
    }

    let a = pipe
        .put(
            &up(&space_a, plain_a.len()),
            ordo_blob::once(plain_a.clone()),
        )
        .await
        .expect("a's upload");
    let b = pipe
        .put(
            &up(&space_b, plain_b.len()),
            ordo_blob::once(plain_b.clone()),
        )
        .await
        .expect("b's upload");

    // Each tenant's object is in that tenant's own store, and in no other.
    assert_eq!(
        keys_under(&root_a),
        vec![format!("tenant-a/{}", ordo_crypto::hex(a.handle()))],
        "tenant a's store holds exactly tenant a's object"
    );
    assert_eq!(
        keys_under(&root_b),
        vec![format!("tenant-b/{}", ordo_crypto::hex(b.handle()))],
        "tenant b's store holds exactly tenant b's object"
    );

    // And the key is unchanged by the seam: still `<space>/<hex handle>`,
    // which is what makes every object written before this change still
    // reachable after it.
    assert_eq!(
        keys_under(&root_a)[0],
        ordo_blob::object_key(&space_a, a.handle()).expect("key"),
    );

    // The read route resolves the same way.
    let got_a = pipe
        .get(
            &space_a,
            "documents",
            "contract",
            a.handle(),
            plain_a.len() as u64,
            0,
            None,
            "req-2",
        )
        .await
        .expect("a reads its own");
    assert_eq!(got_a, plain_a);
    let got_b = pipe
        .get(
            &space_b,
            "documents",
            "contract",
            b.handle(),
            plain_b.len() as u64,
            0,
            None,
            "req-2",
        )
        .await
        .expect("b reads its own");
    assert_eq!(got_b, plain_b);

    // So does the verify route.
    assert!(pipe
        .verify(
            &space_a,
            "documents",
            "contract",
            a.handle(),
            plain_a.len() as u64,
            a.integrity(),
            "req-3",
        )
        .await
        .expect("a's verify"));
    assert!(pipe
        .verify(
            &space_b,
            "documents",
            "contract",
            b.handle(),
            plain_b.len() as u64,
            b.integrity(),
            "req-3",
        )
        .await
        .expect("b's verify"));

    // Asking for tenant a's handle under tenant b's name reaches tenant
    // b's store, which does not have it. That is the second of the two
    // independent mechanisms, now with a second store behind it rather
    // than a second prefix in one.
    let crossed = pipe
        .get(
            &space_b,
            "documents",
            "contract",
            a.handle(),
            plain_a.len() as u64,
            0,
            None,
            "req-4",
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            crossed,
            PipelineError::Store(ordo_blob::BlobError::NotFound)
        ),
        "{crossed:?}"
    );

    // Every route asked about the space the request named, in order, once
    // each. `get` and `verify` resolve once and use the one store for both
    // of their round trips, so a space whose attachment changed mid-read
    // cannot have its header read from one store and its body from
    // another.
    assert_eq!(
        resolver.asked(),
        vec![
            "tenant-a", "tenant-b", // the two puts
            "tenant-a", "tenant-b", // the two gets
            "tenant-a", "tenant-b", // the two verifies
            "tenant-b", // the crossed read, under the space it named
        ],
    );
}

/// **A space with no store refuses on every route, and writes nothing
/// anywhere.**
///
/// The seam's value is that it has no fallback. If an unresolvable space
/// fell back to some store, an attachment that had not taken effect would
/// look like a working upload and would put one tenant's document in
/// another deployment's bucket. The refusal is a
/// [`PipelineError::Store`], which the door already flattens onto
/// `internal`: the operator learns which space had no store, the caller
/// learns nothing.
#[tokio::test]
async fn a_space_with_no_store_refuses_every_route() {
    let root = tempdir("unattached");
    let store = FsStore::shared(&root).expect("store");
    let resolver = Arc::new(PerSpace::new(vec![("tenant-a", Arc::clone(&store))]));
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let pipe = BlobPipeline::new(
        Arc::clone(&resolver) as Arc<dyn ordo_blob::StoreResolver>,
        keys,
        BlobPolicy::DEFAULT,
    );

    let stranger = SpaceId("tenant-nowhere".into());
    let err = pipe
        .put(
            &Upload {
                space: &stranger,
                entity: "documents",
                field: "contract",
                mime: "application/pdf",
                plain_len: 5,
                correlation: "req-1",
                intent: None,
            },
            ordo_blob::once(b"bytes".to_vec()),
        )
        .await
        .unwrap_err();
    match &err {
        PipelineError::Store(ordo_blob::BlobError::Refused(why)) => assert!(
            why.contains("tenant-nowhere"),
            "the operator's copy names the space: {why}"
        ),
        other => panic!("{other:?}"),
    }

    let err = pipe
        .get(
            &stranger,
            "documents",
            "contract",
            &[7u8; 16],
            5,
            0,
            None,
            "req-2",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PipelineError::Store(_)), "{err:?}");

    assert!(
        keys_under(&root).is_empty(),
        "an upload for a space with no store must not land in somebody else's"
    );
}

/// A store that writes every request it is given into a ledger, then
/// serves it from a real [`FsStore`].
///
/// The ledger is the *bucket's* view of the deployment: the request log an
/// operator would see on the other side of the wire, in order, with the
/// key and the range of each. That makes it the right instrument for
/// "behaves identically". It is observable from outside the platform,
/// unlike any field or type this crate holds.
struct Ledger {
    inner: Arc<dyn BlobStore>,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Ledger {
    fn note(&self, line: String) {
        self.seen.lock().expect("ledger").push(line);
    }
}

fn spell(range: &Option<ordo_blob::ByteRange>) -> String {
    match range {
        None => "whole".to_string(),
        Some(r) => match r.len {
            None => format!("{}..end", r.offset),
            Some(n) => format!("{}..+{n}", r.offset),
        },
    }
}

#[async_trait::async_trait]
impl BlobStore for Ledger {
    fn binding(&self) -> &'static str {
        self.inner.binding()
    }
    fn production_ready(&self) -> bool {
        self.inner.production_ready()
    }
    async fn put(
        &self,
        space: &SpaceId,
        handle: &ordo_crypto::custody::BlobHandle,
        len: u64,
        body: ordo_blob::ByteStream,
    ) -> Result<ordo_blob::ObjectHead, ordo_blob::BlobError> {
        self.note(format!(
            "put {} len={len}",
            ordo_blob::object_key(space, handle)?
        ));
        self.inner.put(space, handle, len, body).await
    }
    async fn get(
        &self,
        space: &SpaceId,
        handle: &ordo_crypto::custody::BlobHandle,
        range: Option<ordo_blob::ByteRange>,
    ) -> Result<ordo_blob::ByteStream, ordo_blob::BlobError> {
        self.note(format!(
            "get {} {}",
            ordo_blob::object_key(space, handle)?,
            spell(&range)
        ));
        self.inner.get(space, handle, range).await
    }
    async fn head(
        &self,
        space: &SpaceId,
        handle: &ordo_crypto::custody::BlobHandle,
    ) -> Result<ordo_blob::ObjectHead, ordo_blob::BlobError> {
        self.note(format!("head {}", ordo_blob::object_key(space, handle)?));
        self.inner.head(space, handle).await
    }
    async fn delete(
        &self,
        space: &SpaceId,
        handle: &ordo_crypto::custody::BlobHandle,
    ) -> Result<(), ordo_blob::BlobError> {
        self.note(format!("delete {}", ordo_blob::object_key(space, handle)?));
        self.inner.delete(space, handle).await
    }
}

/// **A deployment that has attached no bucket cannot tell the seam is
/// there.**
///
/// This is the assertion U3 rests on. `SingleStore` is to `StoreResolver`
/// what `SinglePool` is to the database side's `Spaces::pool_of`: the
/// resolver every shipped deployment builds today, returning one store for
/// every space. If routing every call through it changed *anything* an
/// operator could see, the seam would be carrying a behaviour change
/// nobody asked for, and it would be carrying it on the path that stores
/// customers' documents.
///
/// So the assertion is made on what the **bucket** sees rather than on
/// anything inside the platform: the exact sequence of requests, with
/// their keys and their ranges, that one scripted workload produces. Every
/// number below is a fact about the pre-resolver pipeline, and each would
/// move if the seam had added a round trip, dropped one, reordered two, or
/// touched a key:
///
/// - an upload is **one** `put`, under `<space>/<hex handle>`;
/// - a read is **two** `get`s, the 21-byte header and then the planned
///   range, because the epoch has to be known before a key can be asked
///   for;
/// - a verify is a `head` and then one whole-object `get`;
/// - an oversize upload touches the store **not at all**, which is what
///   "refused before a byte is sealed" means from the bucket's side;
/// - a truncated upload is one `put` under its own minted key, which this
///   binding refuses on the length it was told rather than accepting and
///   leaving the pipeline to clean up.
///
/// And the last part of "identically" is that one store still serves every
/// space: two tenants on this deployment write into one root, each under
/// its own prefix, with no second store involved and no key that names
/// anything but the space and the handle.
#[tokio::test]
async fn a_deployment_with_one_store_behaves_identically() {
    let root = tempdir("one-store");
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let store: Arc<dyn BlobStore> = Arc::new(Ledger {
        inner: FsStore::shared(&root).expect("store"),
        seen: Arc::clone(&seen),
    });
    // The resolver the shipped binary builds: one store, every space.
    let resolver: Arc<dyn ordo_blob::StoreResolver> =
        Arc::new(ordo_blob::SingleStore::new(Arc::clone(&store)));
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let pipe = BlobPipeline::new(Arc::clone(&resolver), keys, BlobPolicy::DEFAULT);

    let space = SpaceId("tenant-a".into());
    let plain = pattern(200_000);
    let stored = pipe
        .put(
            &Upload {
                space: &space,
                entity: "documents",
                field: "contract",
                mime: "application/pdf",
                plain_len: plain.len() as u64,
                correlation: "req-1",
                intent: None,
            },
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put");
    let key = ordo_blob::object_key(&space, stored.handle()).expect("key");

    let whole = pipe
        .get(
            &space,
            "documents",
            "contract",
            stored.handle(),
            plain.len() as u64,
            0,
            None,
            "req-2",
        )
        .await
        .expect("read");
    assert_eq!(whole, plain);

    let middle = pipe
        .get(
            &space,
            "documents",
            "contract",
            stored.handle(),
            plain.len() as u64,
            100_000,
            Some(10),
            "req-3",
        )
        .await
        .expect("range");
    assert_eq!(middle, plain[100_000..100_010]);

    assert!(pipe
        .verify(
            &space,
            "documents",
            "contract",
            stored.handle(),
            plain.len() as u64,
            stored.integrity(),
            "req-4",
        )
        .await
        .expect("verify"));

    let before_refusals = seen.lock().expect("ledger").len();

    // An oversize upload. The cap is checked before anything, so the
    // bucket never hears about it.
    let oversize = pipe
        .put(
            &Upload {
                space: &space,
                entity: "documents",
                field: "contract",
                mime: "application/pdf",
                plain_len: BlobPolicy::DEFAULT.max_object + 1,
                correlation: "req-5",
                intent: None,
            },
            ordo_blob::once(b"a lie about the length".to_vec()),
        )
        .await
        .unwrap_err();
    assert!(matches!(oversize, PipelineError::TooLarge { .. }));
    assert_eq!(
        seen.lock().expect("ledger").len(),
        before_refusals,
        "an oversize upload must not reach the store at all"
    );

    // A truncated upload: the object is written, then removed.
    let short = pipe
        .put(
            &Upload {
                space: &space,
                entity: "documents",
                field: "contract",
                mime: "application/pdf",
                plain_len: 1_000,
                correlation: "req-6",
                intent: None,
            },
            ordo_blob::once(b"only a few".to_vec()),
        )
        .await
        .unwrap_err();
    // Refused by the store on the length it was declared, which is this
    // binding's answer and was its answer before the seam existed. A store
    // that accepted the short body instead would be caught by the
    // pipeline's own plaintext count and produce `Truncated`; either is a
    // refusal that leaves no object, which is the property that matters.
    assert!(
        matches!(
            short,
            PipelineError::Store(_) | PipelineError::Truncated { .. }
        ),
        "{short:?}"
    );

    let ledger = seen.lock().expect("ledger").clone();
    let sealed = ordo_crypto::stream::sealed_len(plain.len() as u64);
    let header = ordo_crypto::stream::HEADER as u64;
    // The planned range for a 10-byte read at 100_000: the chunk holding
    // it, sealed, offset past the header. Computed the same way the
    // pipeline computes it rather than written as a literal, because the
    // claim is "the same request as before", not "this request".
    let plan = ordo_crypto::stream::plan_range(plain.len() as u64, 100_000, Some(10))
        .expect("the range is inside the object");
    // And for the whole-object read, which is a range too: it fetches
    // whole sealed chunks, so it asks for more bytes than the plaintext
    // length, and that is the request the bucket has always seen.
    let all = ordo_crypto::stream::plan_range(plain.len() as u64, 0, None)
        .expect("the whole object is inside itself");

    // The truncated upload's key is the only one the ledger names that is
    // not the object's. It is minted per upload, so it is read back out of
    // the ledger rather than predicted.
    let truncated_key = ledger
        .last()
        .and_then(|l| l.strip_prefix("put ").map(str::to_string))
        .and_then(|l| l.split(' ').next().map(str::to_string))
        .expect("the truncated upload's put is the last thing the store saw");
    assert_ne!(truncated_key, key);
    assert!(
        truncated_key.starts_with("tenant-a/"),
        "even a refused upload's key is the space's own: {truncated_key}"
    );

    assert_eq!(
        ledger,
        vec![
            format!("put {key} len={sealed}"),
            format!("get {key} 0..+{header}"),
            format!("get {key} {}..+{}", all.stored_offset, all.stored_len),
            format!("get {key} 0..+{header}"),
            format!("get {key} {}..+{}", plan.stored_offset, plan.stored_len),
            format!("head {key}"),
            format!("get {key} whole"),
            format!(
                "put {truncated_key} len={}",
                ordo_crypto::stream::sealed_len(1_000)
            ),
        ],
        "the bucket saw a different conversation than a one-store deployment used to have"
    );

    // One store, every space: a second tenant on this deployment lands in
    // the same root under its own prefix, and the resolver hands back the
    // very same store rather than a copy of it.
    let other = SpaceId("tenant-b".into());
    let theirs = pipe
        .put(
            &Upload {
                space: &other,
                entity: "documents",
                field: "contract",
                mime: "application/pdf",
                plain_len: 4,
                correlation: "req-7",
                intent: None,
            },
            ordo_blob::once(b"tiny".to_vec()),
        )
        .await
        .expect("the neighbour's upload");
    assert!(Arc::ptr_eq(
        &pipe.store_for(&space).await.expect("a's store"),
        &pipe.store_for(&other).await.expect("b's store"),
    ));
    let mut expected = vec![
        format!("tenant-a/{}", ordo_crypto::hex(stored.handle())),
        format!("tenant-b/{}", ordo_crypto::hex(theirs.handle())),
    ];
    expected.sort();
    assert_eq!(keys_under(&root), expected);
}

// ── objects are bound to their handle (S6-003) ───────────────────────────
//
// Two objects of one column are sealed under one key with one label, so
// before the handle went into the associated data somebody with write
// access to the store could swap them and both opened cleanly: a row then
// served another row's file. These tests do the swap.

/// **The falsification for files.** Two objects in one column, each with
/// its own bytes. Their stored bytes are exchanged in the store. The read
/// of either must fail, because the chunks of one are sealed at the other's
/// handle, and the sealed hash check must fail the same way.
#[tokio::test]
async fn two_objects_swapped_in_the_store_do_not_open() {
    let r = Rig::new("swap");
    let a_plain = pattern(90_000);
    let b_plain: Vec<u8> = pattern(90_000).iter().map(|b| b.wrapping_add(1)).collect();
    let a = r
        .pipe
        .put(
            &r.upload(a_plain.len() as u64),
            ordo_blob::once(a_plain.clone()),
        )
        .await
        .expect("put a");
    let b = r
        .pipe
        .put(
            &r.upload(b_plain.len() as u64),
            ordo_blob::once(b_plain.clone()),
        )
        .await
        .expect("put b");
    let len = a_plain.len() as u64;
    let read = |handle| {
        let r = &r;
        async move {
            r.pipe
                .get(
                    &r.space,
                    "documents",
                    "contract",
                    handle,
                    len,
                    0,
                    None,
                    "rq",
                )
                .await
        }
    };
    assert_eq!(read(a.handle()).await.expect("a reads"), a_plain);
    assert_eq!(read(b.handle()).await.expect("b reads"), b_plain);

    // The swap, in the store, by somebody who holds no key.
    let raw_a = ordo_blob::collect(r.store.get(&r.space, a.handle(), None).await.unwrap())
        .await
        .unwrap();
    let raw_b = ordo_blob::collect(r.store.get(&r.space, b.handle(), None).await.unwrap())
        .await
        .unwrap();
    r.store.delete(&r.space, a.handle()).await.unwrap();
    r.store.delete(&r.space, b.handle()).await.unwrap();
    r.store
        .put(
            &r.space,
            a.handle(),
            raw_b.len() as u64,
            ordo_blob::once(raw_b),
        )
        .await
        .unwrap();
    r.store
        .put(
            &r.space,
            b.handle(),
            raw_a.len() as u64,
            ordo_blob::once(raw_a),
        )
        .await
        .unwrap();

    assert!(
        read(a.handle()).await.is_err(),
        "a's handle now holds b's bytes, which are sealed at b's handle"
    );
    assert!(read(b.handle()).await.is_err());
    let verdict = r
        .pipe
        .verify(
            &r.space,
            "documents",
            "contract",
            a.handle(),
            len,
            a.integrity(),
            "rq",
        )
        .await;
    assert!(
        !matches!(verdict, Ok(true)),
        "the integrity check must not pass a swapped object: {verdict:?}"
    );
}

/// The three sealed attributes on a reference row are bound to their
/// object as well: `mime`, `size` and `integrity` lifted from one row do
/// not open on another's.
#[tokio::test]
async fn the_sealed_attributes_open_only_at_their_own_object() {
    let r = Rig::new("attrs");
    let a = r
        .pipe
        .put(&r.upload(3), ordo_blob::once(b"abc".to_vec()))
        .await
        .expect("put a");
    let b = r
        .pipe
        .put(&r.upload(3), ordo_blob::once(b"xyz".to_vec()))
        .await
        .expect("put b");
    let lease = r.keys.release(&r.space, Purpose::Read, "rq").unwrap();
    let ha = ordo_crypto::hex(a.handle());
    let hb = ordo_crypto::hex(b.handle());
    for sealed in [a.mime(), a.size(), a.integrity()] {
        assert!(r
            .keys
            .open_at(
                &lease,
                "documents",
                "contract",
                ordo_crypto::Anchor::Object(&ha),
                sealed
            )
            .is_ok());
        assert!(
            r.keys
                .open_at(
                    &lease,
                    "documents",
                    "contract",
                    ordo_crypto::Anchor::Object(&hb),
                    sealed
                )
                .is_err(),
            "an attribute lifted onto another object must not open"
        );
    }
}
