//! 18d T2 — **one pool resolver, four consumers**, asserted over the
//! runtime's own source.
//!
//! # Why this is a scan and not a behavioural test
//!
//! The property is *"every surface that reaches a space's rows reaches
//! them through the same resolver"*. The failure it prevents is a surface
//! reading the **wrong database** for a tenant attached elsewhere. That
//! failure was unobservable when this was written, and saying why is the
//! point: no attach op existed, and `$spacedb` was therefore empty in
//! every deployment that could be booted. With one database, four
//! resolvers and one resolver are indistinguishable by every test that can be written, and
//! they stay indistinguishable right up to the first attachment, which is
//! exactly when the difference becomes a tenant's data read out of
//! somebody else's database.
//!
//! So the property is pinned where it *is* observable, which is the
//! construction. A resolver that is never built cannot be the wrong one.
//!
//! **The behavioural half arrived with T4 and this file still earns its
//! place, for one reason.** `tests/attached_tenant.rs` now attaches a
//! tenant to a second real database and proves its writes, reads,
//! security events, idempotency records and `?live` notifications all
//! land there. That is the test this file said should replace it.
//!
//! What it does not cover is the thing this scan is about.
//! `attached_tenant.rs` hand-assembles the resolver, the cache, the drain
//! and the door the way `assemble_program` does, because a subprocess
//! cannot be driven into a two-database state quickly. So it proves the
//! **pieces** compose correctly when wired to one resolver; it cannot
//! prove that the **runtime** wires them that way. A regression that gave
//! the replay cache its own pool inside `assemble_program` would leave
//! `attached_tenant.rs` green.
//!
//! **So the retirement condition is now sharper than it was**: delete
//! this file when a test drives the real boot path against an attached
//! tenant and makes the same five
//! assertions. That needs T8's attach op, so the deployment can be put
//! into that state without a test reaching past it. Until then a scan
//! that outlives its behaviour is a real hazard, and this one has already
//! demonstrated it: T4 moved the construction into a match arm and this
//! file failed on a legitimate edit, which is recorded at the assertion
//! that had to move.

use std::path::Path;

fn runtime_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runtime.rs");
    let body = std::fs::read_to_string(&path).expect("the runtime's source");
    assert!(
        body.len() > 50_000,
        "the scan is reading the wrong file: {} is {} bytes",
        path.display(),
        body.len()
    );
    body
}

/// **The runtime constructs exactly one resolver**, and it is the one
/// `Spaces` holds (T2's done-when, in its own words).
///
/// Before T2 there were four, built independently: for `Spaces`, for the
/// exit and attestation ops, inside `PgReplayCache::new` and inside
/// `SecurityDrain::start`. Each of the last three had a routed variant
/// that nothing called, and `exit_ops.rs`' own doc had said since it was
/// written that it should be handed `Spaces::pools()`.
///
/// Four resolvers over one deployment are four answers to *"which
/// database are this space's rows in"*, and they agree only while every
/// space is on the default pool. They also fail quietly: a pool that
/// answers looks exactly like a pool that answers correctly, so an export
/// reading the control database for a tenant attached elsewhere produces
/// an empty bundle and calls it a disclosure.
#[test]
fn one_resolver_serves_the_door_the_cache_the_drain_and_the_exit_ops() {
    let body = runtime_source();
    // Assembled rather than written literally, so this file never
    // contains the needle it searches for.
    let needle = format!("AttachedPools{}new(", "::");
    let built: Vec<&str> = body
        .lines()
        .filter(|l| l.contains(&needle))
        .map(|l| l.trim())
        .collect();
    assert_eq!(
        built.len(),
        1,
        "the runtime builds {} pool resolvers. T2's done-when is that it \
         builds one, the one `Spaces` holds, and that every other surface \
         is handed `spaces.pools()`. Found:\n{}",
        built.len(),
        built.join("\n")
    );
    // **And never a single pool** (ruled 2026-10-06). Every tenant has a
    // database of its own, so the runtime has no branch that serves every
    // space from one pool. A `SinglePool` built here would be either a
    // second resolver or that branch come back.
    let single = format!("SinglePool{}new(", "::");
    assert!(
        !body.contains(&single),
        "the runtime builds a single-pool resolver again"
    );

    // And the one that is left is the deployment's resolver, not some
    // other surface that happens to want a pool.
    let at = body.find(&needle).expect("the one construction");
    let before = &body[at.saturating_sub(200)..at];
    assert!(
        before.contains("let resolver"),
        "the attached resolver is no longer the deployment's resolver, which \
         is the only place it belongs. Context:\n{before}"
    );
    // The chosen resolver is what `Spaces` is built with, wherever the
    // arms put it. This is the half that did not weaken.
    assert!(
        body.contains("Spaces::routed(deployment.clone(), resolver, witness.clone())"),
        "`Spaces` is no longer built from the resolver this function chose"
    );
}

/// **The three consumers are handed the resolver rather than building
/// one**, which is the other half of the same property: counting
/// constructions proves nothing if a consumer simply stopped resolving.
#[test]
fn the_cache_the_drain_and_the_ops_are_handed_the_one_resolver() {
    let body = runtime_source();
    for (what, needle) in [
        ("the replay cache", "PgReplayCache::routed("),
        ("the security drain", "SecurityDrain::start_routed("),
        ("the platform ops", "pools: spaces.pools()"),
    ] {
        assert!(
            body.contains(needle),
            "{what} does not take the deployment's resolver: `{needle}` is \
             not in the runtime. Either it went back to building its own, \
             or it stopped resolving at all, and the second is worse \
             because nothing about it looks wrong."
        );
    }
    // The non-routed constructors are what this increment exists to stop
    // calling. They stay in their own crates for embedders with one
    // database; the runtime is not one of those.
    for gone in ["PgReplayCache::new(", "SecurityDrain::start("] {
        assert!(
            !body.contains(gone),
            "the runtime calls `{gone}` again, which builds a resolver of \
             its own over the control pool"
        );
    }
}

/// **The same property for the bytes** (18e U5): the runtime builds one
/// store resolver, chosen by `ORDO_BLOB_ROUTING` and by nothing else, and
/// every consumer is handed that one.
///
/// A scan for the same reason the pool version is one. **U6 registered the
/// attach op (2026-09-24) and this scan is still the only thing that proves
/// this property**, which is worth stating rather than leaving as a stale
/// note. A door test can now reach `std.space.attachBucket`, but it reaches
/// it through `PlatformOps` with a hand-wired `BucketOps`; driving a real
/// boot into a two-bucket state needs a second reachable S3 target, and U5
/// records why there is none (no local S3, the contract suite skips without
/// `ORDO_BLOB_S3_*`, and §2 B-C4 forbids `fs:` as an attachment target).
/// `ordo-exec`'s `tests/bucket_routing.rs` proves the resolver resolves;
/// nothing but this can prove the runtime is the thing holding it.
///
/// What a regression here looks like: the door keeping a `SingleStore` of
/// its own while the sweep and the exit ops take the attached one. Every
/// test stays green, every upload for an attached tenant goes to the
/// deployment's bucket, and the sweep then walks the tenant's bucket and
/// finds nothing to collect, which reads as a healthy sweep.
///
/// Delete this when a test drives a real boot with
/// `ORDO_BLOB_ROUTING=attached` against a tenant attached to a second
/// reachable target.
#[test]
fn one_store_resolver_serves_the_door_the_export_the_sweep_and_the_erase() {
    let body = runtime_source();
    // Assembled, so this file never contains the needles it searches for.
    let single = format!("SingleStore{}new(", "::");
    let attached = format!("AttachedStores{}new(", "::");
    for (what, needle) in [("single-store", &single), ("attached", &attached)] {
        let built = body.lines().filter(|l| l.contains(needle)).count();
        assert_eq!(
            built, 1,
            "the runtime builds {built} {what} resolvers. It builds one, in \
             one `match`, and hands that one to the pipeline, the exit ops \
             and the sweep."
        );
    }
    // And both are arms of the same choice, not two independent sites.
    let at = body.find(&single).expect("the single construction");
    let to = body.find(&attached).expect("the attached construction");
    let (lo, hi) = (at.min(to), at.max(to));
    assert!(
        hi - lo < 1_200,
        "the two store resolvers are no longer arms of one choice: they are \
         {} characters apart",
        hi - lo
    );
    // The pipeline takes the chosen resolver rather than a store.
    assert!(
        body.contains("ordo_blob::BlobPipeline::new(\n                stores.clone(),"),
        "the pipeline is no longer built from the resolver this function chose"
    );
}
