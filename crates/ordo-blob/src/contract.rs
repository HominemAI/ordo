//! The store contract, as one suite both bindings run.
//!
//! Two bindings invite "two behaviours". The answer is not to argue that
//! they behave alike — it is to make the **trait's contract** the test
//! surface and run the same assertions against both, with the S3 run gated
//! on a live endpoint the way `ordo-conformance` already gates its
//! Postgres tiers. A binding that fails one of these is a binding this
//! platform does not support, whatever it says on its box.
//!
//! Each case names the platform behaviour that rests on it, because a
//! contract test whose failure message is "put/get mismatch" tells an
//! implementer nothing about what they broke.
//!
//! Living in `src/` rather than `tests/` is deliberate: an integration
//! test cannot be run by a *downstream* binding, and the next binding
//! (GCS, a KMS-fronted proxy, a BYO-bucket wrapper) has to be able to run
//! this suite from its own crate. `ordo-conformance` makes the same choice
//! for the same reason.

use ordo_crypto::custody::{BlobHandle, SpaceId};

use crate::{collect, object_key, once, BlobError, BlobStore, ByteRange};

/// A deterministic handle for a case, so a failure names the object.
fn handle(n: u8) -> BlobHandle {
    let mut h = [0u8; 16];
    h[0] = n;
    h[15] = 0xa5;
    h
}

/// Run the whole suite. Returns the case names that ran, so a caller can
/// assert the suite did not silently shrink.
pub async fn run(store: &dyn BlobStore, space: &SpaceId) -> Vec<&'static str> {
    let mut ran = Vec::new();
    macro_rules! case {
        ($name:literal, $body:expr) => {{
            let r: Result<(), BlobError> = $body.await;
            r.unwrap_or_else(|e| panic!("[{}] {}: {e}", store.binding(), $name));
            ran.push($name);
        }};
    }

    case!("round trip", round_trip(store, space));
    case!("empty object", empty_object(store, space));
    case!(
        "absent reads are NotFound",
        absent_is_not_found(store, space)
    );
    case!("overwrite is refused", overwrite_refused(store, space));
    case!("delete is idempotent", delete_is_idempotent(store, space));
    case!("range in the middle", range_middle(store, space));
    case!("range to the end", range_open(store, space));
    case!("range past the end", range_past_end(store, space));
    case!("head reports the length", head_length(store, space));
    case!("a short body is refused", short_body_refused(store, space));
    case!(
        "spaces do not share keys",
        spaces_are_disjoint(store, space)
    );
    // U0 · promoted from a unit test in `lib.rs` (found while specifying
    // per-tenant buckets, 2026-09-21) so every binding's own conformance
    // run — including the S3 run gated on a live endpoint — asserts the
    // key's shape for itself, rather than trusting one binding-agnostic
    // check to stand for all of them.
    case!(
        "the_object_key_is_the_space_and_the_handle_and_nothing_else",
        object_key_is_space_and_handle_only(space)
    );
    ran
}

/// The ordinary path. If this fails nothing else means anything.
async fn round_trip(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(1);
    let body: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let head = s.put(sp, &h, body.len() as u64, once(body.clone())).await?;
    assert_eq!(head.bytes, body.len() as u64);
    let got = collect(s.get(sp, &h, None).await?).await?;
    assert_eq!(got, body, "the bytes that came back are not the bytes");
    s.delete(sp, &h).await
}

/// A zero-byte upload is a legal upload. It has to be: a sealed empty
/// file is still an AEAD envelope with a tag, and a store that refused
/// length zero would make "the user uploaded an empty file" an error class
/// the language has no word for.
async fn empty_object(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(2);
    s.put(sp, &h, 0, once(Vec::new())).await?;
    assert_eq!(collect(s.get(sp, &h, None).await?).await?, Vec::<u8>::new());
    assert_eq!(s.head(sp, &h).await?.bytes, 0);
    s.delete(sp, &h).await
}

/// The three reads of an absent key all answer `NotFound` — and not
/// `Refused`, which an operator would page on.
async fn absent_is_not_found(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(3);
    assert_eq!(s.head(sp, &h).await.unwrap_err(), BlobError::NotFound);
    // A `get` may fail at the call or at the first chunk, depending on
    // whether the binding learns of the absence before or after it hands
    // back a stream. Both are the contract; neither may succeed.
    let err = match s.get(sp, &h, None).await {
        Err(e) => e,
        Ok(body) => collect(body).await.expect_err("a stream over nothing"),
    };
    assert_eq!(err, BlobError::NotFound);
    Ok(())
}

/// **The invariant the whole addressing scheme rests on.** Handles are
/// keyed-random and minted per upload, so a second `put` under one handle
/// is a repeated CSPRNG seed inside one space-epoch, or a bug. Either way
/// the honest answer is a refusal: an overwrite would destroy a document
/// that a live row still points at, and it would do it silently.
async fn overwrite_refused(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(4);
    s.put(sp, &h, 3, once(b"one".to_vec())).await?;
    let second = s.put(sp, &h, 3, once(b"two".to_vec())).await;
    assert_eq!(
        second.unwrap_err(),
        BlobError::AlreadyExists,
        "a second put under one handle must refuse, not overwrite"
    );
    assert_eq!(collect(s.get(sp, &h, None).await?).await?, b"one".to_vec());
    s.delete(sp, &h).await
}

/// **What makes phase 2 of the sweep crash-safe.** The tombstone is
/// stamped *after* the object is deleted, so a crash between the two
/// leaves the sweep to retry a delete that has already happened. A store
/// that refused would turn a safe retry into a permanently stuck
/// tombstone.
async fn delete_is_idempotent(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(5);
    s.put(sp, &h, 1, once(b"x".to_vec())).await?;
    s.delete(sp, &h).await?;
    s.delete(sp, &h).await?;
    s.delete(sp, &handle(0xee)).await
}

/// **What a progressive download and a resumable one both need**, and what
/// `f7-vault-plan.md` records as absent until chunked STREAM lands. A
/// store that cannot serve an interior range makes range reads a lie one
/// layer up.
async fn range_middle(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(6);
    let body: Vec<u8> = (0..1000u32).map(|i| (i % 256) as u8).collect();
    s.put(sp, &h, 1000, once(body.clone())).await?;
    let got = collect(s.get(sp, &h, Some(ByteRange::new(400, Some(100)))).await?).await?;
    assert_eq!(got, body[400..500], "the range is off by something");
    s.delete(sp, &h).await
}

async fn range_open(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(7);
    let body: Vec<u8> = (0..300u32).map(|i| (i % 256) as u8).collect();
    s.put(sp, &h, 300, once(body.clone())).await?;
    let got = collect(s.get(sp, &h, Some(ByteRange::new(290, None))).await?).await?;
    assert_eq!(got, body[290..]);
    s.delete(sp, &h).await
}

/// Asking past the end is a refusal, not an empty success. An empty
/// success is how a truncated download reads as a complete one.
async fn range_past_end(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(8);
    s.put(sp, &h, 10, once(vec![7u8; 10])).await?;
    let err = match s.get(sp, &h, Some(ByteRange::new(64, Some(8)))).await {
        Err(e) => e,
        Ok(body) => collect(body).await.expect_err("a range past the end"),
    };
    assert_eq!(err, BlobError::Range);
    s.delete(sp, &h).await
}

/// `head` is what `$blobref."bytes"` is checked against, and what a
/// re-seal worker counts. It must agree with the object, not with the
/// caller's belief about it.
async fn head_length(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(9);
    s.put(sp, &h, 12345, once(vec![3u8; 12345])).await?;
    assert_eq!(s.head(sp, &h).await?.bytes, 12345);
    s.delete(sp, &h).await
}

/// A body shorter than its declared length is a truncated upload, and a
/// truncated upload must never become a successful `put`: the object would
/// satisfy a `head`, fail its AEAD tag, and read as corruption rather than
/// as an upload that did not finish.
async fn short_body_refused(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(10);
    let r = s.put(sp, &h, 100, once(vec![1u8; 10])).await;
    assert!(
        matches!(r, Err(BlobError::Refused(_))),
        "a short body was accepted: {r:?}"
    );
    // And it left nothing behind that a later read could find.
    assert_eq!(s.head(sp, &h).await.unwrap_err(), BlobError::NotFound);
    Ok(())
}

/// **The key carries no metadata beyond the space and the handle.**
/// Promoted from a unit test on `object_key` itself (found while
/// specifying per-tenant buckets, 2026-09-21): the function is pure and
/// shared by both bindings, so proving its shape once, in `lib.rs`, proved
/// it for neither binding's own conformance run in particular. Every
/// extra component a key could carry — an entity, a field, a date, an
/// epoch — is a fact a bucket listing discloses to someone holding no
/// keys, which is exactly what `lib.rs`'s module doc argues against.
async fn object_key_is_space_and_handle_only(sp: &SpaceId) -> Result<(), BlobError> {
    let h = handle(12);
    let k = object_key(sp, &h)?;
    assert_eq!(
        k,
        format!("{}/{}", sp.0, ordo_crypto::hex(&h)),
        "the key is not exactly the space and the handle"
    );
    assert_eq!(
        k.matches('/').count(),
        1,
        "no entity, field or date path — every extra component is a fact a bucket \
         listing discloses to someone holding no keys"
    );
    Ok(())
}

/// The second of the two independent mechanisms that scope a handle: the
/// derivation is keyed per space, *and* the key is prefixed per space. A
/// caller who somehow learned another space's handle reads nothing.
async fn spaces_are_disjoint(s: &dyn BlobStore, sp: &SpaceId) -> Result<(), BlobError> {
    let other = SpaceId(format!("{}-other", sp.0));
    let h = handle(11);
    s.put(sp, &h, 4, once(b"mine".to_vec())).await?;
    assert_eq!(s.head(&other, &h).await.unwrap_err(), BlobError::NotFound);
    s.delete(&other, &h).await?;
    assert_eq!(
        collect(s.get(sp, &h, None).await?).await?,
        b"mine".to_vec(),
        "another space's delete reached into this one"
    );
    s.delete(sp, &h).await
}

#[cfg(test)]
mod cross_store {
    //! **U0's third pinned property.** `spaces_are_disjoint` above proves
    //! that two spaces inside *one* store's key namespace do not collide.
    //! This proves the property one level up: two *stores*, given the
    //! identical space id and the identical handle, do not answer for one
    //! another either. `object_key` is pure and shared by both bindings,
    //! so nothing in the key format tells one store instance from
    //! another — isolation is entirely the accident of `FsStore::root`
    //! (and, on the S3 binding, the endpoint and bucket) being separate
    //! fields on separate values, and nothing before this test asserted
    //! that. It is worth pinning because the failure mode is exactly the
    //! shape a future per-tenant resolver risks: a cache or a lookup keyed
    //! by object key alone, rather than by store identity (`docs/plan-
    //! tenant-storage.md` §10 B-X1's `Attachment::identity` cache is the
    //! production answer, not yet built), would pass every other test
    //! here and silently let one store serve another's bytes.

    use super::*;
    use crate::fs::FsStore;

    fn tempdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ordo-blob-contract-{name}-{}-{}",
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
    async fn spaces_with_different_stores_do_not_share_keys() {
        let dir_a = tempdir("a");
        let dir_b = tempdir("b");
        let a = FsStore::new(&dir_a).unwrap();
        let b = FsStore::new(&dir_b).unwrap();
        let space = SpaceId("tenant-a".into());
        let h = handle(20);

        a.put(&space, &h, 4, once(b"mine".to_vec())).await.unwrap();

        // The identical space id and the identical handle, asked of the
        // *other* store: nothing about the key told them apart, so the
        // only thing that kept them apart was the store itself.
        assert_eq!(
            b.head(&space, &h).await.unwrap_err(),
            BlobError::NotFound,
            "a second store answered for the first store's object"
        );

        assert_eq!(
            collect(a.get(&space, &h, None).await.unwrap())
                .await
                .unwrap(),
            b"mine".to_vec(),
            "the first store's own object should be unaffected by the second store \
             existing at all"
        );

        a.delete(&space, &h).await.unwrap();
        std::fs::remove_dir_all(&dir_a).ok();
        std::fs::remove_dir_all(&dir_b).ok();
    }
}
