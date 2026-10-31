//! 11-27, **the isolation suite at the door**: two tenants, one process,
//! and what naming a space buys.
//!
//! `ordo-conformance/tests/spaces.rs` (9h) seeds one fixture into two
//! spaces and proves the storage layer keeps them apart;
//! `ordo-exec/tests/spaces.rs` does the same one layer up, over the revert
//! applier, the chain, the collector and the security stream. This file is
//! the layer above both.
//!
//! # What changed, and what this file used to say
//!
//! It used to open by explaining that its twin was a header rather than a
//! tenant: `ordo_host::SPACE` was a constant, `deployment_engine`
//! hardcoded it, and **no in-tree path provisioned a second space into a
//! running deployment**. That was true and is not any more. The host
//! resolves an executor, a fixture and a chain per space
//! ([`ordo_host::spaces`]) and [`ordo_host::provision`] is the path that
//! brings one into existence, so the twin here is a second tenant the
//! deployment actually serves and the harness builds it the way a
//! deployment would.
//!
//! Two consequences for how to read the tests below. The claim is no
//! longer "claiming a space buys exactly that space's access", which a
//! single-space host could satisfy by refusing everything else; it is
//! **each of two served tenants reaches its own rows, its own chain and
//! its own catalog, and neither reaches the other's**. And a refusal is
//! now evidence rather than a foregone conclusion: the second space
//! exists, is provisioned, and answers, so a request that is denied in it
//! was denied by the check under test.
//!
//! The harness therefore does what a deployment does, in that order:
//! provision the twin (custody, then its founding owner), then build the
//! access engine from the `members` table, which is where the twin now is.
//! Nothing is patched into the engine afterwards.
//!
//! # What each test is for
//!
//! | test                                          | the statement it pins                          |
//! |-----------------------------------------------|------------------------------------------------|
//! | `two_tenants_read_only_their_own_rows`        | the resolver on the op path, end to end        |
//! | `each_tenants_writes_join_its_own_chain`      | one `WriteLog` per space, never one shared     |
//! | `two_spaces_resolve_to_two_executors`         | what the resolver hands back, and caches       |
//! | `an_unprovisioned_space_is_refused_twice_over`| the door's denial and the resolver's under it  |
//! | `the_catalog_answers_for_the_space_named`     | `catalog_route`'s per-op access check          |
//! | `an_upload_cannot_name_another_space`         | the blob door's write-side space binding       |
//! | `provisioning_the_same_space_twice_is_quiet`  | the retry an interrupted provision completes   |
//! | `every_space_resolves_to_the_default_pool`    | S1: attaching nothing changes nothing          |
//! | `a_space_is_served_from_the_database_it_resolves_to` | S1: writes and reads follow the resolver |
//! | `a_space_that_moves_is_cut_again`             | the view cache follows the resolver's answer   |
//! | `a_restarted_host_continues_the_chain`        | 11-28: writes reach `$commandlog`, ids unique  |
//! | `a_space_on_an_unreachable_database_is_refused_and_others_are_not` | S3: failure at resolve |
//!
//! `deployment.rs` already pins D0's three reserved surfaces
//! (`a_platform_op_is_not_authorized_by_a_caller_named_space`,
//! `metrics_denies_an_owner_of_another_space`,
//! `the_alerts_route_answers_the_deployment_owner`) and `blob_door.rs`
//! pins the read side's non-enumerability
//! (`a_handle_in_another_space_is_not_found_like_an_absent_one`). Neither
//! is repeated here.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request};
use ordo_api::blob::{BlobLimits, BlobPort, BlobState};
use ordo_api::{router_with_blobs, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_blob::{BlobPipeline, BlobPolicy, BlobStore, FsStore};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_crypto::SpaceId;
use ordo_exec::route::PoolResolver;
use ordo_exec::Pool;
use ordo_host::blob::PlatformBlobs;
use ordo_host::deploy::Deployment;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::Value;
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"host-spaces-test-secret";

/// The deployment's own space, and the fixture's: the tenant the seed
/// writes and the one the reserved surfaces authorize against.
const HERE: &str = ordo_host::DEFAULT_SPACE;
/// The second tenant, provisioned through [`ordo_host::provision`] rather
/// than assembled by hand. It is served, not merely named.
const TWIN: &str = "spc_twin";

/// `mem_o` is Owner and `mem_1` Manager in `HERE`, by the fixture's seed.
const OWNER: &str = "mem_o";
/// `TWIN`'s founding owner, and a member of nothing in `HERE`.
///
/// The member id and the principal are the same string, which is the
/// fixture's own convention (`$caller` is compared against a `-> members`
/// column, and the seed makes `members.id` the principal a token proves).
const TWIN_OWNER: &str = "mem_twin";

/// `blob_door.rs`'s entity, unchanged: one blob field, and a guard that
/// admits a row's own owner alone.
const DOCS_SCHEMA: &str = "
entity docs (key: doc) {
  owner -> members
  title:  text
  scan:   blob?
  access read   owner == $caller
  access write  owner == $caller
}
";

struct Harness {
    app: axum::Router,
    pool: ordo_exec::Pool,
    /// The resolver both doors share, so a test can read the chain a
    /// request appended to and ask what a space resolves to.
    spaces: Arc<ordo_host::spaces::Spaces>,
    /// The provider provisioning minted custody through, so a test can
    /// re-run the provision the way a retry does.
    keys: Arc<dyn ordo_crypto::KeyProvider>,
    dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

async fn harness(db: &str) -> Harness {
    harness_with(db, |_| None).await
}

/// [`harness`], with the pool resolver `route` builds from the harness's own
/// pool, or the default when it builds none.
async fn harness_with(
    db: &str,
    route: impl FnOnce(&Pool) -> Option<Arc<dyn PoolResolver>>,
) -> Harness {
    let src = format!("{FIXTURE}\n{DOCS_SCHEMA}");
    let f = Arc::new(Fixture::from_source(&src, SEED).expect("the fixture plus docs compiles"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");

    // `Pool::connect` runs `ensure_platform_tables`, which is what creates
    // `$blobref`/`$blobtomb`.
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");

    let deployment = Deployment::pinned(
        "host-spaces-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );

    // **The twin, provisioned before the door is built.** Custody and a
    // founding owner, through the one entry point, with nothing patched
    // into the engine afterwards, which is what makes every refusal below
    // a refusal by a check rather than by the absence of a tenant.
    let provisioned = ordo_host::provision::provision_space(
        &pool,
        f.keys.as_ref(),
        TWIN,
        &ordo_host::provision::FoundingOwner {
            member_id: TWIN_OWNER,
            principal: TWIN_OWNER,
            display_name: "The Twin's owner",
        },
    )
    .await
    .expect("the twin provisions");
    assert_eq!(
        provisioned,
        ordo_host::provision::Provisioned::Created {
            space: TWIN.to_string()
        },
        "a freshly provisioned space reports as created, so a harness that \
         silently reused a leftover one would say so"
    );

    // Built AFTER provisioning, and from the `members` table, which is
    // where provisioning put the twin's owner. The deployment's own space
    // and the twin therefore arrive by the same path.
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");

    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let access = Arc::new(ordo_host::IdentityAccess {
        engine: Arc::new(engine),
        key: TokenKey::from_secret(SECRET),
        now: Box::new(|| NOW),
        stream: Arc::new(ordo_secevent::Recorder::default()),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });

    let dir = std::env::temp_dir().join(format!("ordo-host-spaces-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    // **One resolver, shared.** The op path and the blob door must append
    // to the same chain per space; two resolvers would be two chains for
    // one tenant, which is the mistake the shared field exists to prevent.
    let spaces = match route(&pool) {
        Some(pools) => ordo_host::spaces::Spaces::routed(
            deployment.clone(),
            pools,
            ordo_exec::witness::Witness::new(pool.clone()),
        ),
        None => ordo_host::spaces::Spaces::new(deployment.clone()),
    };
    let port = Arc::new(PlatformBlobs {
        spaces: spaces.clone(),
        pipeline: BlobPipeline::new(
            Arc::new(ordo_blob::SingleStore::new(store)),
            f.keys.clone(),
            BlobPolicy::DEFAULT,
        ),
    });

    let state = ApiState {
        registry: deployment.handle(),
        // The real executor, not a stub. What used to make that pointless
        // was that only one space could reach it at all.
        executor: Arc::new(ordo_host::FixtureExecutor {
            spaces: spaces.clone(),
            pg: pg.clone(),
            page_size: 50,
        }),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"host-spaces-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };

    let platform = ordo_host::serve::PlatformState::new(
        access,
        deployment,
        pool.clone(),
        f.keys.clone(),
        // The deployment's own space as configuration, which is what it
        // now is: `HERE` is the default this reads back, and the reserved
        // surfaces authorize against it rather than against a space a
        // caller names.
        ordo_host::deployment_space().expect("the default is a bindable space id"),
        metrics,
    )
    // 17aa: the resolver readiness reads its quarantine register from. The
    // same `Arc` the door resolves through, so what readiness reports is
    // what the door found rather than a second opinion.
    .with_spaces(spaces.clone())
    // The store the blob door above is built over, told to readiness as
    // well. Without this the harness reports `config` failed for a
    // deployment that does have a store, which is a harness that
    // contradicts its own wiring.
    .with_blob_label(Some(format!("fs:{}", dir.display())));

    let app = router_with_blobs(
        state,
        BlobState {
            port: port as Arc<dyn BlobPort>,
            limits: BlobLimits::DEFAULT,
        },
    )
    .merge(ordo_host::serve::platform_router(platform));

    Harness {
        app,
        pool,
        spaces,
        keys: f.keys.clone(),
        dir,
    }
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn get_in(app: &axum::Router, path: &str, space: &str, who: &str) -> (u16, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", space)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// An unauthenticated GET, for the two health routes that take no token.
async fn get_json(app: &axum::Router, path: &str) -> (u16, Value) {
    // The deployment owner's token: `/health/ready` gives its detail to the
    // owner alone (audit F12).
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("authorization", format!("Bearer {}", token(OWNER)))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn put_scan(app: &axum::Router, id: &str, space: &str, who: &str) -> (u16, Value) {
    let body = b"a signed contract".to_vec();
    let req = Request::builder()
        .method(Method::PUT)
        .uri(format!("/blob/docs/scan/{id}"))
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", space)
        .header("content-length", body.len().to_string())
        .header("content-type", "application/pdf")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A `members` row, written **after** the engine was built from the same
/// table, so the identity layer does not know about it.
///
/// That split is the test's whole instrument. `docs.owner -> members` is a
/// composite foreign key, so a row owned by the twin's owner needs a member
/// row beside it; the row guard then compares `owner == $caller` and
/// passes. The door, which asks the `AccessEngine`, still knows nothing of
/// this principal in this space. So a refusal can only come from the space
/// binding, and a success can only mean the binding was dropped.
async fn insert_member(pool: &ordo_exec::Pool, space: &str, id: &str) {
    pool.rows(&format!(
        "INSERT INTO \"members\" (\"$space\", \"id\", \"principal\", \"displayName\", \"kind\", \
         \"memberCount\", \"role\", \"status\", \"joined\") \
         VALUES ('{space}', '{id}', '{id}', '{id}', 'principal', 1, 'owner', 'active', \
         DATE '2026-01-01')"
    ))
    .await
    .expect("insert member");
}

async fn insert_doc(pool: &ordo_exec::Pool, space: &str, id: &str, owner: &str) {
    pool.rows(&format!(
        "INSERT INTO \"docs\" (\"$space\", \"id\", \"rev\", \"owner\", \"title\") \
         VALUES ('{space}', '{id}', 1, '{owner}', 'a doc')"
    ))
    .await
    .expect("insert doc");
}

// ── the catalog, D0's mirror image ──────────────────────────────────────

/// **The surfaces that are right to take the caller's word still have to
/// act on it.** D0 was a surface reading `space_of` where it owed
/// `state.space`; this is the same seam from the other side, and it fails
/// the same way.
///
/// `catalog_route` and `interface_route` are `space_of`'s only legitimate
/// callers: they answer "which ops can this caller reach in the space
/// they named", which genuinely has the caller's space in it. `authorize`'s
/// own documentation says so. What makes that safe is that the per-op
/// check is made **against the named space**: a route that read the space
/// from the header and then checked against `state.space` would hand
/// anybody who can reach the deployment a list of every op it serves,
/// which is the disclosure the catalog's access filter exists to prevent.
///
/// Both directions, because "the twin's owner sees nothing here" and
/// "each sees only their own" are different claims and only the second one
/// is isolation:
///
/// - the deployment's owner naming `TWIN` sees an empty catalog,
///   though they hold everything **here**;
/// - the twin's owner naming `HERE` sees an empty catalog, though they
///   hold every one of these op names **there**;
/// - and each, naming their own space, sees a non-empty one, because
///   otherwise an empty catalog would be the answer to everything and the
///   first two assertions would prove nothing at all.
#[tokio::test(flavor = "multi_thread")]
async fn the_catalog_answers_for_the_space_named() {
    let h = harness("ordo_host_spaces_catalog").await;

    let (st, mine) = get_in(&h.app, "/catalog", HERE, OWNER).await;
    assert_eq!(st, 200);
    let n = mine.as_array().map(|a| a.len()).unwrap_or(0);
    assert!(n > 0, "the owner reaches ops in their own space: {mine}");

    let (st, theirs) = get_in(&h.app, "/catalog", TWIN, OWNER).await;
    assert_eq!(st, 200);
    assert_eq!(
        theirs,
        serde_json::json!([]),
        "the deployment's own owner holds nothing in a space they are not a \
         member of, and the catalog must say so rather than answering about \
         the space the process happens to serve"
    );

    let (st, theirs) = get_in(&h.app, "/catalog", HERE, TWIN_OWNER).await;
    assert_eq!(st, 200);
    assert_eq!(
        theirs,
        serde_json::json!([]),
        "and the twin's owner, who holds every one of these op names in their \
         own space, reaches none of them here"
    );

    let (st, twin_own) = get_in(&h.app, "/catalog", TWIN, TWIN_OWNER).await;
    assert_eq!(st, 200);
    assert_eq!(
        twin_own.as_array().map(|a| a.len()),
        Some(n),
        "and the twin's own grants are real, from the same profile the \
         deployment ships. Without this the two empty answers above would be \
         the route failing, not the check working"
    );

    // The same question through the well-known path, which is a second
    // route onto the same handler and so a second place to forget.
    let (st, well_known) = get_in(&h.app, "/.well-known/ordo-catalog", TWIN, OWNER).await;
    assert_eq!(st, 200);
    assert_eq!(well_known, serde_json::json!([]));
}

// ── the blob door's write side ──────────────────────────────────────────

/// **An upload names a space, and naming one buys exactly that space.**
///
/// `blob_door.rs` pins the read side: a handle whose `$blobref` row lives
/// in another space is `not_found`, told apart from an absent one by
/// nothing. The write side had no equivalent, and it is the side with more
/// to lose: a `PUT` carries bytes, mints a handle and writes a reference
/// row, all three stamped with the space the header named
/// (`PlatformBlobs::write`, `let space = SpaceId(req.space.clone())`).
///
/// # The case that distinguishes, and why the obvious one does not
///
/// There was an asymmetry inside `write` worth aiming at, and it is the
/// one this increment closed. The space the sealer and the `$blobref` row
/// take comes from the **request header**; the converge that sets the
/// entity column used to run through an executor whose `space` was fixed
/// when the deployment was built. In a single-space deployment those two
/// coincide, so a test where they would disagree had to be constructed.
/// They now come from one resolution of the same binding, and this test is
/// what keeps them there.
///
/// The obvious construction proves nothing. The twin's owner naming `TWIN`
/// against a row owned by somebody else answers `not_found` whichever
/// space was consulted: absent in `TWIN`, and invisible to them in
/// `HERE`. Both readings give 404, so 404 says nothing.
///
/// So `doc_2` is owned **by the twin's owner, in `HERE`**. Now the two
/// readings diverge cleanly:
///
/// - bound to the header's space, the row is not in `TWIN` and the
///   upload is `not_found`;
/// - bound to a space the executor chose for itself, the row is in `HERE`
///   and its owner is the caller, so the guard passes and the bytes land:
///   a caller with no membership in the space being served writing a row
///   in it, on the strength of a header naming somewhere else.
///
/// The refusal is checked with the `$blobref` table as well as the status,
/// because a reference row stamped `TWIN` beside a column written in
/// `HERE` is the half-landed shape a status code alone would not show.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_cannot_name_another_space() {
    let h = harness("ordo_host_spaces_upload").await;
    insert_doc(&h.pool, HERE, "doc_1", OWNER).await;
    insert_member(&h.pool, HERE, TWIN_OWNER).await;
    insert_doc(&h.pool, HERE, "doc_2", TWIN_OWNER).await;

    // The control: the owner uploading into their own space, to their own
    // row. Everything below is the difference between this call and that.
    let (st, body) = put_scan(&h.app, "doc_1", HERE, OWNER).await;
    assert_eq!(st, 204, "the ordinary upload must work: {body}");

    // The case that distinguishes: the twin owner's own row in `HERE`,
    // where they are not a member, reached under the space they do own.
    let (st, body) = put_scan(&h.app, "doc_2", TWIN, TWIN_OWNER).await;
    assert_ne!(
        st, 204,
        "the upload bound the space it was told to bind and found no row \
         there; a 204 here is a write into the deployment's space by a \
         caller with no membership in it: {body}"
    );

    // And the deployment's own owner, naming a space they are not in: the
    // door refuses before a row is ever looked for.
    let (st, body) = put_scan(&h.app, "doc_1", TWIN, OWNER).await;
    assert_ne!(
        st, 204,
        "holding everything here is not authority in a space named by a \
         header: {body}"
    );

    // Nothing landed in the space the two refused calls named, and the
    // twin owner's row in `HERE` still holds no object.
    let count =
        |space: &str| format!("SELECT count(*) FROM \"$blobref\" WHERE \"$space\" = '{space}'");
    assert_eq!(
        h.pool.rows(&count(TWIN)).await.expect("count")[0][0].as_deref(),
        Some("0"),
        "a refused upload must not leave a reference row in the space it named"
    );
    assert_eq!(
        h.pool.rows(&count(HERE)).await.expect("count")[0][0].as_deref(),
        Some("1"),
        "and exactly the one the control upload wrote is here, so the \
         refusals did not land a column in this space either"
    );
    let scan = h
        .pool
        .rows(&format!(
            "SELECT \"scan\" FROM \"docs\" WHERE \"$space\" = '{HERE}' AND \"id\" = 'doc_2'"
        ))
        .await
        .expect("select");
    assert_eq!(
        scan[0][0], None,
        "the twin owner's row in this space names no object"
    );
}

// ── two tenants, one process ────────────────────────────────────────────

/// **The claim this whole increment is for**: two spaces, both provisioned
/// through the real path, both served by one process, each reaching its
/// own rows through the real router and neither reaching the other's.
///
/// Before the host held an executor per space this could not be written at
/// all. The door bound the space, the access check was made against it,
/// and then `FixtureExecutor::execute` compared it against a constant and
/// answered `internal` for anything else, so a second tenant's read was a
/// 500 no matter what they were entitled to. The old module doc said a
/// real two-tenant test was not constructible, and this is the test that
/// stopped being true.
///
/// `std.docs.list` is the op because it is derived rather than authored:
/// nothing about it was written with spaces in mind, so it exercises what
/// every op gets rather than what one op was built to survive.
///
/// Both directions and both outcomes, because a suite where the twin only
/// ever gets refused would pass on a host that refuses the twin
/// everything:
///
/// - each owner, in their own space, sees their own row and only it;
/// - each owner, in the other space, is refused by the door;
/// - and the two rows carry the same shape and differ only in the space
///   they are stamped with, so a statement that lost its predicate would
///   return both.
#[tokio::test(flavor = "multi_thread")]
async fn two_tenants_read_only_their_own_rows() {
    let h = harness("ordo_host_spaces_tenants").await;

    // The same row id in both spaces. 9h's argument, applied here: real
    // ids do not collide, "and that is precisely why an id-only test
    // proves nothing": a statement missing its space predicate finds the
    // right row by id and looks correct forever.
    insert_doc(&h.pool, HERE, "doc_1", OWNER).await;
    insert_doc(&h.pool, TWIN, "doc_1", TWIN_OWNER).await;
    insert_doc(&h.pool, TWIN, "doc_2", TWIN_OWNER).await;

    let ids = |body: &Value| -> Vec<String> {
        body["rows"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| r["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    let (st, mine) = get_in(&h.app, "/op/std.docs.list", HERE, OWNER).await;
    assert_eq!(st, 200, "the deployment's own tenant reads: {mine}");
    assert_eq!(
        ids(&mine),
        ["doc_1"],
        "one row here, and not the twin's two: {mine}"
    );

    let (st, theirs) = get_in(&h.app, "/op/std.docs.list", TWIN, TWIN_OWNER).await;
    assert_eq!(
        st, 200,
        "**the second tenant is served**, by the same router, the same \
         registry and the same executor type as the first: {theirs}"
    );
    assert_eq!(
        ids(&theirs),
        ["doc_2", "doc_1"],
        "the twin's own two rows, newest id first per `std.E.list`'s sort, \
         and neither space's `doc_1` leaking into the other's page: {theirs}"
    );

    // Each owner in the other's space. Refused at the door, which is where
    // it should be refused: the executor is not the access check, and a
    // deployment that leant on the executor to keep tenants apart would be
    // one membership bug away from a cross-tenant read.
    let (st, body) = get_in(&h.app, "/op/std.docs.list", TWIN, OWNER).await;
    assert_eq!(
        st, 403,
        "owning the deployment's own space is not membership in another: {body}"
    );
    let (st, body) = get_in(&h.app, "/op/std.docs.list", HERE, TWIN_OWNER).await;
    assert_eq!(
        st, 403,
        "and the twin's owner holds nothing here, though they hold this very \
         op there: {body}"
    );
}

/// **One chain per space, and the door and the op path on the same one.**
///
/// `FixtureExecutor` used to hold a single `Arc<Mutex<WriteLog>>` whose own
/// comment said why it was shared: "the command chain is per space, and
/// each op's group appends under the same head". That is exactly right for
/// one space and exactly wrong for two. One shared log makes both tenants'
/// groups append under one head, which is two spaces' history in one
/// chain and unsplittable afterwards.
///
/// An upload is the write used because it reaches the chain through the
/// blob door rather than through the executor, so the test also pins the
/// other half: the two doors resolve the *same* log for a space. A second
/// resolver would pass every isolation assertion here and still be two
/// chains for one tenant.
#[tokio::test(flavor = "multi_thread")]
async fn each_tenants_writes_join_its_own_chain() {
    let h = harness("ordo_host_spaces_chains").await;
    insert_doc(&h.pool, HERE, "doc_1", OWNER).await;
    insert_doc(&h.pool, TWIN, "doc_1", TWIN_OWNER).await;

    // Counted with `len_in`, not `len`: `WriteLog::len` answers for the
    // conformance space alone, because "the chain is per space … there is
    // no total across the deployment to ask for". The store was already
    // multi-space; what was not was the host holding one log per tenant.
    let count = |log: &Arc<std::sync::Mutex<ordo_conformance::WriteLog>>, space: &str| {
        log.lock()
            .unwrap()
            .len_in(&ordo_commandlog::SpaceId(space.to_string()))
    };

    let here = h.spaces.log_of(HERE);
    let twin = h.spaces.log_of(TWIN);
    assert!(
        !Arc::ptr_eq(&here, &twin),
        "two spaces must not share a write log: one log is one chain head, \
         and one head for two tenants cannot be split back apart"
    );

    let (st, body) = put_scan(&h.app, "doc_1", HERE, OWNER).await;
    assert_eq!(st, 204, "{body}");
    assert_eq!(count(&here, HERE), 1, "one upload, one group here");
    assert_eq!(
        count(&twin, TWIN),
        0,
        "and nothing at all in the twin's chain, which has seen no write"
    );

    let (st, body) = put_scan(&h.app, "doc_1", TWIN, TWIN_OWNER).await;
    assert_eq!(st, 204, "the twin uploads to its own row: {body}");
    assert_eq!(
        (count(&here, HERE), count(&twin, TWIN)),
        (1, 1),
        "one group each, and the second upload did not land on the first \
         tenant's head"
    );
    assert_eq!(
        (count(&here, TWIN), count(&twin, HERE)),
        (0, 0),
        "and neither log holds a group for the other's space, which is what \
         a single shared log would have produced while every count above \
         still read correctly"
    );
}

/// What [`ordo_host::spaces::Spaces::resolve`] hands back, stated directly
/// rather than inferred from the rows a request saw.
///
/// Three properties, and each of them is something the request path relies
/// on without being able to check:
///
/// 1. **Two spaces are two executors**, each bound to its own space. The
///    space is a field on `ordo_exec::Executor` because *"keys are per
///    space, so this is the isolation boundary rather than a label"*, and
///    that is only true if the field actually differs per tenant.
/// 2. **The fixture is bound too.** The write protocol reads its space off
///    the `Fixture`, not off the executor, so a view that rebound one and
///    not the other would plan reads in one tenant and write rows in
///    another.
/// 3. **Resolving twice is the same view and the same log**, which is what
///    keeps "one chain per space" true across requests and keeps the
///    per-space cut from being paid on every call.
#[tokio::test(flavor = "multi_thread")]
async fn two_spaces_resolve_to_two_executors() {
    let h = harness("ordo_host_spaces_resolver").await;

    let a = h
        .spaces
        .resolve(HERE)
        .await
        .expect("the deployment's own space");
    let b = h.spaces.resolve(TWIN).await.expect("the provisioned twin");

    assert_eq!(a.view.exec.space.0, HERE);
    assert_eq!(b.view.exec.space.0, TWIN);
    assert_eq!(a.view.fixture.space().0, HERE);
    assert_eq!(
        b.view.fixture.space().0,
        TWIN,
        "the fixture carries the binding the write protocol reads, so it \
         must move with the executor's"
    );

    let again = h.spaces.resolve(HERE).await.expect("resolving twice");
    assert!(
        Arc::ptr_eq(&a.view, &again.view),
        "one view per space per version: a fresh cut per request would pay \
         a schema clone on every call and would still be correct, which is \
         why nothing downstream would notice it regressing"
    );
    assert!(
        Arc::ptr_eq(&a.log, &again.log),
        "and one chain head per space, which is the half that would NOT \
         still be correct"
    );
}

/// **A space nothing provisioned is refused, and refused twice over.**
///
/// The door refuses first: an unprovisioned space has no membership, so
/// the access check denies, and it denies with the same `access_denied` a
/// member of the wrong space gets, because a distinct "no such space" would
/// enumerate the deployment's tenants.
///
/// The resolver refuses under it. That second refusal is the one worth
/// pinning, because it is the one that still holds if an access checker is
/// ever wrong: without it, a space that reached the executor would be
/// served on a chain and a fixture minted out of a request header, with no
/// keys behind either.
#[tokio::test(flavor = "multi_thread")]
async fn an_unprovisioned_space_is_refused_twice_over() {
    let h = harness("ordo_host_spaces_unprovisioned").await;

    let Err(refused) = h.spaces.resolve("spc_nobody").await else {
        panic!(
            "a space with no custody must not resolve: minting a view and a \
             chain for it is minting a tenant from a string"
        );
    };
    let message = refused.to_string();
    assert!(
        message.contains("spc_nobody") && message.contains("custody"),
        "the refusal names what is missing, for an operator reading a log: \
         {message}"
    );

    // And through the door, where it must be indistinguishable from being
    // denied in a space that does exist.
    let (unknown_st, unknown) = get_in(&h.app, "/op/std.docs.list", "spc_nobody", OWNER).await;
    let (known_st, known) = get_in(&h.app, "/op/std.docs.list", TWIN, OWNER).await;
    assert_eq!(unknown_st, 403, "{unknown}");
    assert_eq!(
        (unknown_st, unknown["error"]["code"].clone()),
        (known_st, known["error"]["code"].clone()),
        "a space that does not exist and a space this caller is not in must \
         answer identically, or the door is an existence oracle over the \
         deployment's tenants"
    );
}

/// **Provisioning is safe to run again**, which is what makes an
/// interrupted one completable rather than a half-space to clean up.
///
/// Three runs, and each is a state a crash could have left behind:
///
/// 1. the same call the harness already made, which must report the space
///    as existing rather than creating a second owner beside the first;
/// 2. the same call after the founding row is gone, custody without it,
///    which is exactly what a crash between the two steps leaves, which
///    must complete the space rather than refuse it as half-built;
/// 3. a name the door could never bind, which must write nothing at all.
#[tokio::test(flavor = "multi_thread")]
async fn provisioning_the_same_space_twice_is_quiet() {
    use ordo_host::provision::{provision_space, FoundingOwner, ProvisionRefusal, Provisioned};

    let h = harness("ordo_host_spaces_provision").await;
    let owner = FoundingOwner {
        member_id: TWIN_OWNER,
        principal: TWIN_OWNER,
        display_name: "The Twin's owner",
    };
    let count =
        |space: &str| format!("SELECT count(*) FROM \"members\" WHERE \"$space\" = '{space}'");

    let again = provision_space(&h.pool, h.keys.as_ref(), TWIN, &owner)
        .await
        .expect("a second provision of a live space is not an error");
    assert_eq!(
        again,
        Provisioned::AlreadyExists {
            space: TWIN.to_string()
        },
        "and it says so, rather than reading as a space that was just created"
    );
    assert_eq!(
        h.pool.rows(&count(TWIN)).await.expect("count")[0][0].as_deref(),
        Some("1"),
        "one founding owner, not two"
    );

    // The interrupted shape: custody minted, the row never written. The
    // space is inert in that state (nobody is a member, so nobody can
    // reach it), and a retry is what completes it.
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("delete");
    let completed = provision_space(&h.pool, h.keys.as_ref(), TWIN, &owner)
        .await
        .expect("a retry completes an interrupted provision");
    assert_eq!(
        completed,
        Provisioned::Created {
            space: TWIN.to_string()
        },
        "the founding row is what makes the space exist, so writing it is \
         what the retry had left to do"
    );

    let refused = provision_space(&h.pool, h.keys.as_ref(), "not a space id", &owner)
        .await
        .expect_err("a name the door cannot bind is refused");
    assert!(
        matches!(refused, ProvisionRefusal::Malformed(_)),
        "and refused for the name, before custody: {refused}"
    );
    assert_eq!(
        h.pool
            .rows("SELECT count(*) FROM \"members\" WHERE \"$space\" = 'not a space id'")
            .await
            .expect("count")[0][0]
            .as_deref(),
        Some("0"),
        "nothing is written for a space that could never be reached"
    );
}

// ── routing (plan-space-routing S1) ─────────────────────────────────────

/// **A deployment that attaches nothing routes every space to the pool it
/// booted with**, which is `plan-space-routing.md`'s claim that the seam
/// changes nothing on the day it lands.
///
/// Asked of the handle rather than inferred from rows, because both spaces
/// live in one database here and a test that read rows would pass whichever
/// pool answered.
#[tokio::test(flavor = "multi_thread")]
async fn every_space_resolves_to_the_default_pool() {
    let h = harness("ordo_host_spaces_default_pool").await;

    // A space nothing provisioned routes too, because routing is not
    // existence: custody answers that, and `resolve` asks it first.
    // The handle the default answers with is the booted pool's connections
    // **charged to the space** (20a): `pool_of` is the one resolution every
    // request-path checkout passes through, so it is where a checkout
    // acquires a name and a share. `same_handle` compares the charge, so
    // the comparison is made against a handle charged the same way.
    for space in [HERE, TWIN, "spc_never_provisioned"] {
        let pool = h
            .spaces
            .pool_of(space)
            .await
            .expect("the default routes every space");
        assert_eq!(pool.charged_space(), Some(space), "the checkout has a name");
        assert!(
            pool.same_handle(&h.pool.for_space(&SpaceId(space.to_string()))),
            "`{space}` resolved to a pool other than the one the deployment booted with"
        );
    }
    for space in [HERE, TWIN] {
        let bound = h
            .spaces
            .resolve(space)
            .await
            .expect("a provisioned space resolves");
        assert!(
            bound
                .view
                .exec
                .pool
                .same_handle(&h.pool.for_space(&SpaceId(space.to_string()))),
            "`{space}`'s executor was cut on a pool other than the default"
        );
    }
}

/// An observer that records nothing. It exists to make a second pool
/// handle over the same connections: [`ordo_exec::Pool::same_handle`]
/// compares observers as well as sockets, so this is how a test asks for
/// "a different pool" without a second database.
struct Silent;

impl ordo_exec::metrics::AppendObserver for Silent {
    fn lock_wait(&self, _space: &str, _seconds: f64) {}
    fn append_work(&self, _space: &str, _seconds: f64) {}
}

/// The twin on one pool handle, every other space on another.
struct TwinApart {
    default: Pool,
    twin: Pool,
}

impl PoolResolver for TwinApart {
    fn resolve<'a>(&'a self, space: &'a SpaceId) -> ordo_exec::route::Resolving<'a> {
        let pool = if space.0 == TWIN {
            self.twin.clone()
        } else {
            self.default.clone()
        };
        Box::pin(async move { Ok(pool) })
    }

    fn targets<'a>(&'a self) -> ordo_exec::route::Targets<'a> {
        let (default, twin) = (self.default.clone(), self.twin.clone());
        Box::pin(async move {
            Ok(vec![
                ordo_exec::route::Target {
                    label: "default".into(),
                    pool: Ok(default),
                },
                ordo_exec::route::Target {
                    label: "twin".into(),
                    pool: Ok(twin),
                },
            ])
        })
    }
}

/// A blob read through the door, in `space`.
async fn get_blob_in(
    app: &axum::Router,
    handle_hex: &str,
    space: &str,
    who: &str,
) -> (u16, Vec<u8>) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/blob/{handle_hex}"))
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", space)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

async fn scan_of(pool: &Pool, space: &str, id: &str) -> Option<String> {
    pool.rows(&format!(
        "SELECT \"scan\" FROM \"docs\" WHERE \"$space\" = '{space}' AND \"id\" = '{id}'"
    ))
    .await
    .expect("select scan")
    .into_iter()
    .next()
    .and_then(|r| r.into_iter().next())
    .flatten()
}

async fn blobrefs_in(pool: &Pool, space: &str) -> String {
    pool.rows(&format!(
        "SELECT count(*) FROM \"$blobref\" WHERE \"$space\" = '{space}'"
    ))
    .await
    .expect("count references")
    .into_iter()
    .next()
    .and_then(|r| r.into_iter().next())
    .flatten()
    .expect("a count")
}

/// **A space's rows are in the database its space resolves to**, for both
/// halves of the door.
///
/// The twin is routed to a second database and the deployment's own space
/// is not. The twin's upload has to land there (the write, through
/// `Spaces::resolve`) and its read has to be answered from there (the read,
/// through `Spaces::pool_of`). A door still reading through one pool fails
/// both: the upload finds no row to attach to, because the twin's row is
/// only in the second database, and the read finds no reference.
///
/// Both databases carry the program, because a target owes the schema. Only
/// the default carries the twin's membership as the door sees it, because
/// the door authorizes against the control plane rather than the target;
/// that is the split S4 makes formal, and this test already relies on it.
#[tokio::test(flavor = "multi_thread")]
async fn a_space_is_served_from_the_database_it_resolves_to() {
    const ELSEWHERE: &str = "ordo_host_spaces_elsewhere";
    let src = format!("{FIXTURE}\n{DOCS_SCHEMA}");
    let f = Fixture::from_source(&src, SEED).expect("the fixture plus docs compiles");
    f.reset(&Pg::named(ELSEWHERE))
        .expect("the twin's database is seeded");
    let elsewhere = Pool::connect(ordo_exec::PoolConfig {
        db: ELSEWHERE.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("a pool on the twin's database");

    let h = harness_with("ordo_host_spaces_routed", {
        let elsewhere = elsewhere.clone();
        move |pool| {
            Some(Arc::new(TwinApart {
                default: pool.clone(),
                twin: elsewhere,
            }) as Arc<dyn PoolResolver>)
        }
    })
    .await;

    // The twin's row exists only in the twin's database, and the
    // deployment's own row only in the default.
    insert_member(&elsewhere, TWIN, TWIN_OWNER).await;
    insert_doc(&elsewhere, TWIN, "doc_1", TWIN_OWNER).await;
    insert_doc(&h.pool, HERE, "doc_1", OWNER).await;

    let (st, body) = put_scan(&h.app, "doc_1", TWIN, TWIN_OWNER).await;
    assert_eq!(
        st, 204,
        "the twin uploads to the row in its own database: {body}"
    );
    let (st, body) = put_scan(&h.app, "doc_1", HERE, OWNER).await;
    assert_eq!(
        st, 204,
        "and the deployment's own space to the default: {body}"
    );

    assert_eq!(
        (
            blobrefs_in(&elsewhere, TWIN).await,
            blobrefs_in(&h.pool, TWIN).await
        ),
        ("1".to_string(), "0".to_string()),
        "the twin's reference belongs in the twin's database and nowhere else"
    );
    assert_eq!(
        blobrefs_in(&elsewhere, HERE).await,
        "0",
        "the twin's database took a reference for the deployment's own space"
    );

    let handle = scan_of(&elsewhere, TWIN, "doc_1")
        .await
        .expect("the upload set the twin's column in the twin's database");
    let (st, bytes) = get_blob_in(&h.app, &handle, TWIN, TWIN_OWNER).await;
    assert_eq!(
        st, 200,
        "the twin's read did not find its reference, so the read path is not routed"
    );
    assert_eq!(bytes, b"a signed contract");
}

/// **A space the resolver moves is cut again on its next request.**
///
/// The per-space view caches an executor, and the executor carries a pool,
/// so a cache keyed by space alone would pin a space to the database it was
/// first served from for the life of the version. The owner ruled that the
/// routing must be able to carry a move (`routing-migration`, 2026-09-12),
/// and this is the part of a move the cache owes: a different answer from
/// the resolver produces a different executor, on the new pool.
#[tokio::test(flavor = "multi_thread")]
async fn a_space_that_moves_is_cut_again() {
    let h = harness("ordo_host_spaces_moved").await;
    let live = h.spaces.deployment().current();
    let elsewhere = h.pool.observing(Arc::new(Silent));

    let first = live.in_space(TWIN, h.pool.clone());
    let again = live.in_space(TWIN, h.pool.clone());
    assert!(
        Arc::ptr_eq(&first, &again),
        "an unchanged answer should reuse the cut rather than clone the schema again"
    );

    let moved = live.in_space(TWIN, elsewhere.clone());
    assert!(
        !Arc::ptr_eq(&first, &moved),
        "a space answered with a different pool kept the executor cut on the old one"
    );
    assert!(moved.exec.pool.same_handle(&elsewhere));
    assert_eq!(
        moved.space, first.space,
        "and a move changes the pool, not the space"
    );
}

// ── the durable chain (checklist 11-28) ─────────────────────────────────

async fn chain_of(pool: &Pool, space: &str) -> Vec<(i64, String, String)> {
    pool.rows(&format!(
        "SELECT \"idx\", \"correlation\", \"actor\" FROM \"$commandlog\" \
         WHERE \"space\" = '{space}' ORDER BY \"idx\""
    ))
    .await
    .expect("read the chain")
    .into_iter()
    .map(|r| {
        let cell = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        (cell(0).parse().expect("an index"), cell(1), cell(2))
    })
    .collect()
}

/// **A host that restarts continues each space's chain, and mints nothing
/// it minted before.**
///
/// Two resolvers over one deployment and one database are two processes in
/// sequence: each holds its own write logs, and neither remembers what the
/// other minted. Before 11-28 this failed twice over. The host's writes
/// never reached `$commandlog` at all, and the write log minted ids from a
/// counter that starts at one in every process, so the second process's
/// first create minted the first process's row id, and once appends are
/// durable its first group would mint a correlation the chain already
/// holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_host_continues_the_chain() {
    let h = harness("ordo_host_spaces_restart").await;
    let before = chain_of(&h.pool, HERE).await.len();

    let write = |spaces: Arc<ordo_host::spaces::Spaces>, name: &'static str| async move {
        let bound = spaces.resolve(HERE).await.expect("resolves");
        bound
            .view
            .exec
            .write_chained(
                bound.view.fixture.clone(),
                bound.log.clone(),
                format!("addRootCategory($name: \"{name}\")"),
                OWNER.to_string(),
                None,
            )
            .await
    };

    write(h.spaces.clone(), "before_restart")
        .await
        .expect("the first process writes");
    let restarted = ordo_host::spaces::Spaces::new(h.spaces.deployment().clone());
    write(restarted, "after_restart")
        .await
        .expect("the second process writes, and the chain takes its group");

    let chain = chain_of(&h.pool, HERE).await;
    assert_eq!(
        chain.len(),
        before + 2,
        "both writes are in the durable chain: {chain:?}"
    );
    let (a, b) = (&chain[before], &chain[before + 1]);
    assert_eq!(
        b.0,
        a.0 + 1,
        "the second process continued the chain rather than forking it"
    );
    assert_ne!(a.1, b.1, "two processes minted the same correlation");
    assert_eq!(a.2, format!("user:{OWNER}"), "the actor is the caller");

    let ids: Vec<String> = h
        .pool
        .rows(&format!(
            "SELECT \"id\" FROM \"categories\" WHERE \"$space\" = '{HERE}' \
             AND \"name\" IN ('before_restart', 'after_restart')"
        ))
        .await
        .expect("read the categories")
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    assert_eq!(ids.len(), 2, "both creates landed: {ids:?}");
    assert_ne!(ids[0], ids[1], "two processes minted the same row id");
}

// ── attached databases (plan-space-routing S3) ──────────────────────────

/// **A space whose database cannot be reached is refused before anything is
/// planned, and no other space notices.**
///
/// The twin is attached to a port nothing listens on. Its row also exists in
/// the default database, which is the point of the second assertion: a
/// resolver that fell back to the default on a failed dial would serve the
/// twin from the wrong database and the upload would succeed. It must not.
/// The deployment's own space, on the default, keeps working throughout.
#[tokio::test(flavor = "multi_thread")]
async fn a_space_on_an_unreachable_database_is_refused_and_others_are_not() {
    let h = harness_with("ordo_host_spaces_unreachable", |pool| {
        Some(Arc::new(ordo_exec::route::AttachedPools::new(
            pool.clone(),
            pool.clone(),
            ordo_exec::PoolConfig::from_env(),
            Arc::new(ordo_exec::route::Installed::new().passwordless("nowhere")),
        )) as Arc<dyn PoolResolver>)
    })
    .await;
    ordo_exec::spacedb::ensure_table(&h.pool)
        .await
        .expect("the attachment table");
    ordo_exec::spacedb::attach(
        &h.pool,
        &SpaceId(TWIN.into()),
        &ordo_exec::spacedb::DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            user: "ordo".into(),
            db: "nowhere".into(),
            credential: "nowhere".into(),
            custody: ordo_exec::spacedb::DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: TWIN_OWNER.into(),
        },
    )
    .await
    .expect("attached");
    insert_doc(&h.pool, TWIN, "doc_1", TWIN_OWNER).await;
    insert_doc(&h.pool, HERE, "doc_1", OWNER).await;

    let refused = h.spaces.resolve(TWIN).await;
    assert!(
        matches!(
            refused,
            Err(ordo_host::spaces::SpaceRefusal::Unroutable { .. })
        ),
        "an unreachable database is refused at resolve"
    );

    let (st, _) = put_scan(&h.app, "doc_1", TWIN, TWIN_OWNER).await;
    assert_eq!(st, 500, "the twin's upload cannot be served from anywhere");
    assert_eq!(
        scan_of(&h.pool, TWIN, "doc_1").await,
        None,
        "and it was not served from the default database instead"
    );

    let (st, body) = put_scan(&h.app, "doc_1", HERE, OWNER).await;
    assert_eq!(st, 204, "another space is unaffected: {body}");
}

// ── R2d-iii: continuity at resolve ──────────────────────────────────────

/// Append `n` synthetic groups to a space's chain.
async fn seed_chain(pool: &ordo_exec::Pool, space: &str, n: usize) {
    for i in 0..n {
        let g = ordo_commandlog::CommandGroup {
            correlation: ordo_commandlog::CorrelationId(format!("{space}_cg_{i}")),
            actor: ordo_commandlog::Actor::User("usr_r2d".into()),
            commands: vec![ordo_commandlog::Command {
                id: ordo_commandlog::CommandId(format!("{space}_cmd_{i}")),
                space: ordo_commandlog::SpaceId(space.to_string()),
                entity: "docs".into(),
                row_id: format!("doc_{i}"),
                kind: ordo_commandlog::CommandKind::Create,
                before: None,
                after: Some(ordo_commandlog::Image::new()),
                at: ordo_commandlog::Timestamp(1_000 + i as u64),
            }],
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        };
        let tx = pool.begin().await.unwrap();
        tx.append_group(&g).await.expect("append");
        tx.commit().await.unwrap();
    }
}

/// **A tenant whose history moved backwards is refused at `resolve`, and
/// served again once the restore is recorded.**
///
/// The refusal is what makes a silent restore impossible to serve through;
/// lifting it without a restart is what keeps a recorded restore from being
/// an outage. Only passing verdicts are cached, which is what makes both
/// true at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_rewound_tenant_is_refused_at_resolve_and_served_after_the_record() {
    let h = harness("ordo_host_spaces_rewound").await;
    let witness = ordo_exec::witness::Witness::new(h.pool.clone());
    let space = ordo_commandlog::SpaceId(TWIN.into());
    witness.ensure_table().await.unwrap();
    h.pool.rows("DELETE FROM \"$chainwitness\"").await.unwrap();

    seed_chain(&h.pool, TWIN, 4).await;
    let signer = ordo_commandlog::Ed25519Signer::from_seed("spaces-key", &[81u8; 32]);
    ordo_exec::undo::sign_checkpoint(
        &h.pool,
        &space,
        &signer,
        ordo_commandlog::Timestamp(400),
        &witness,
    )
    .await
    .expect("a checkpoint");

    // It serves, and the check ran exactly once for it.
    let before = h.spaces.continuity_checks();
    h.spaces.resolve(TWIN).await.expect("a continuous space");
    h.spaces.resolve(TWIN).await.expect("still continuous");
    h.spaces.resolve(TWIN).await.expect("still continuous");
    assert_eq!(
        h.spaces.continuity_checks(),
        before + 1,
        "once per process for a space that passes"
    );

    // The restore, of the space's own tables: `$checkpoints` goes back with
    // the chain, so nothing in this database disagrees with anything.
    h.pool
        .rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = '{TWIN}' AND \"idx\" >= 2"
        ))
        .await
        .unwrap();
    h.pool
        .rows(&format!(
            "DELETE FROM \"$checkpoints\" WHERE \"space\" = '{TWIN}'"
        ))
        .await
        .unwrap();

    // The already-passing cache still serves this process; a fresh resolver
    // over the same deployment is what a restarted replica is.
    let fresh = ordo_host::spaces::Spaces::routed(
        h.spaces.deployment().clone(),
        h.spaces.pools().clone(),
        witness.clone(),
    );
    let Err(refused) = fresh.resolve(TWIN).await else {
        panic!("a rewound tenant resolved");
    };
    let message = refused.to_string();
    assert!(
        matches!(
            refused,
            ordo_host::spaces::SpaceRefusal::Rewound { ref space, .. } if space == TWIN
        ),
        "{message}"
    );
    assert!(
        message.contains("control database"),
        "the refusal says which record it read: {message}"
    );

    // Recorded, and the very next request is served — no restart.
    ordo_exec::undo::record_restore(
        &h.pool,
        &witness,
        &space,
        ordo_exec::undo::RestoreRecord {
            operator: "ops@example.test".into(),
            source: ordo_commandlog::RestoreSource::Pitr {
                at: ordo_commandlog::Timestamp(1_757_000_000),
            },
            pre_restore_head: None,
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("recorded");
    fresh
        .resolve(TWIN)
        .await
        .expect("a recorded restore is served, without a restart");
}

/// **One rewound tenant is a quarantine, not an outage** (17aa).
///
/// The owner, 2026-09-21: *"The system is the control plane and should
/// function without teeth into those. Dependence is on the control plane,
/// not the deployment underneath."* So the three facts belong in one test,
/// because each is worthless without the other two: the rewound tenant does
/// not serve, every other tenant does, and readiness says which space is
/// held closed rather than leaving an operator to find out from a customer.
///
/// **Observed failing first:** `/health/ready` carried no `quarantined`
/// field at all, so the third assertion read `null`.
#[tokio::test(flavor = "multi_thread")]
async fn a_rewound_tenant_is_quarantined_while_the_deployment_keeps_serving() {
    let h = harness("ordo_host_spaces_quarantine").await;
    let witness = ordo_exec::witness::Witness::new(h.pool.clone());
    let space = ordo_commandlog::SpaceId(TWIN.into());
    witness.ensure_table().await.unwrap();
    h.pool.rows("DELETE FROM \"$chainwitness\"").await.unwrap();

    insert_doc(&h.pool, HERE, "doc_1", OWNER).await;
    insert_doc(&h.pool, TWIN, "doc_1", TWIN_OWNER).await;

    seed_chain(&h.pool, TWIN, 4).await;
    let signer = ordo_commandlog::Ed25519Signer::from_seed("quarantine-key", &[83u8; 32]);
    ordo_exec::undo::sign_checkpoint(
        &h.pool,
        &space,
        &signer,
        ordo_commandlog::Timestamp(400),
        &witness,
    )
    .await
    .expect("a checkpoint");

    // The restore of the twin's own tables, and nothing recording it.
    h.pool
        .rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = '{TWIN}' AND \"idx\" >= 2"
        ))
        .await
        .unwrap();
    h.pool
        .rows(&format!(
            "DELETE FROM \"$checkpoints\" WHERE \"space\" = '{TWIN}'"
        ))
        .await
        .unwrap();

    // 1 — the rewound tenant does not serve. The control plane staying up
    // is what makes the repair possible, never permission to serve data
    // whose history is in question.
    let (st, body) = get_in(&h.app, "/op/std.docs.list", TWIN, TWIN_OWNER).await;
    assert_eq!(
        st, 500,
        "a rewound tenant served its rows through the door: {body}"
    );

    // 2 — and every other tenant does, through the same router and the
    // same resolver.
    let (st, mine) = get_in(&h.app, "/op/std.docs.list", HERE, OWNER).await;
    assert_eq!(
        st, 200,
        "one tenant's rewound chain took another tenant down with it: {mine}"
    );

    // 3 — readiness is ready, and names the space that is held closed.
    let (st, ready) = get_json(&h.app, "/health/ready").await;
    assert_eq!(
        st, 200,
        "a quarantined tenant made the whole deployment not-ready: {ready}"
    );
    assert_eq!(ready["status"], "ready", "{ready}");
    let held: Vec<&str> = ready["quarantined"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["space"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(
        held,
        [TWIN],
        "readiness must name the space an operator has to repair: {ready}"
    );
    let why = ready["quarantined"][0]["reason"]
        .as_str()
        .unwrap_or_default();
    assert!(
        why.contains("no longer reaches"),
        "and say what it found: {ready}"
    );

    // Recorded, the restore is lawful history: the next request is served
    // and readiness holds nothing closed, with no restart in between.
    ordo_exec::undo::record_restore(
        &h.pool,
        &witness,
        &space,
        ordo_exec::undo::RestoreRecord {
            operator: "ops@example.test".into(),
            source: ordo_commandlog::RestoreSource::Pitr {
                at: ordo_commandlog::Timestamp(1_757_000_000),
            },
            pre_restore_head: None,
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("recorded");

    let (st, body) = get_in(&h.app, "/op/std.docs.list", TWIN, TWIN_OWNER).await;
    assert_eq!(st, 200, "a recorded restore serves again: {body}");
    let (_, ready) = get_json(&h.app, "/health/ready").await;
    assert_eq!(
        ready["quarantined"].as_array().map(Vec::len),
        Some(0),
        "and the quarantine lifts with it: {ready}"
    );
}

/// A space nothing has ever recorded a head for has nothing to fire on. The
/// check exists to catch a chain that moved backwards **from a head somebody
/// signed**, and a young space has no such head.
#[tokio::test(flavor = "multi_thread")]
async fn a_space_with_no_record_is_never_refused() {
    let h = harness("ordo_host_spaces_norecord").await;
    let witness = ordo_exec::witness::Witness::new(h.pool.clone());
    witness.ensure_table().await.unwrap();
    h.pool.rows("DELETE FROM \"$chainwitness\"").await.unwrap();

    seed_chain(&h.pool, TWIN, 4).await;
    // A rewind with nothing signed over it: no checkpoint, no witness row.
    h.pool
        .rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = '{TWIN}' AND \"idx\" >= 2"
        ))
        .await
        .unwrap();

    let fresh = ordo_host::spaces::Spaces::routed(
        h.spaces.deployment().clone(),
        h.spaces.pools().clone(),
        witness,
    );
    fresh
        .resolve(TWIN)
        .await
        .expect("nothing claims this chain was ever longer");
}

// ── 18d T5: quarantine covers unreachable, and the caller never learns why
// ──────────────────────────────────────────────────────────────────────
//
// `a_space_on_an_unreachable_database_is_refused_and_others_are_not` above
// pins S3's half of this (the write does not land somewhere else). These
// three pin what T5 adds on top of the same refusal: it is registered, it
// answers 503 rather than 500, and the caller never reads the dial error
// that produced it.

/// **An unreachable target quarantines its tenant and answers 503, not
/// 500** (18d R-F1 / R-F2). Before this increment `SpaceRefusal::Unroutable`
/// mapped to `ExecError::internal` and the quarantine register had exactly
/// one writer, `continuity_of`, which an unroutable target never reaches
/// because routing fails before the continuity read is attempted. So a
/// tenant whose cluster was down failed every request as an opaque 500
/// while `/health/ready` reported nothing at all — an operator's first
/// evidence was a customer.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_target_quarantines_its_tenant_and_answers_503() {
    let h = harness_with("ordo_host_spaces_t5_unreachable", |pool| {
        Some(Arc::new(ordo_exec::route::AttachedPools::new(
            pool.clone(),
            pool.clone(),
            ordo_exec::PoolConfig::from_env(),
            Arc::new(ordo_exec::route::Installed::new().passwordless("t5-nowhere")),
        )) as Arc<dyn PoolResolver>)
    })
    .await;
    ordo_exec::spacedb::ensure_table(&h.pool)
        .await
        .expect("the attachment table");
    ordo_exec::spacedb::attach(
        &h.pool,
        &SpaceId(TWIN.into()),
        &ordo_exec::spacedb::DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            user: "ordo".into(),
            db: "t5_nowhere".into(),
            credential: "t5-nowhere".into(),
            custody: ordo_exec::spacedb::DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: TWIN_OWNER.into(),
        },
    )
    .await
    .expect("attached");

    let (st, body) = get_in(&h.app, "/op/std.docs.list", TWIN, TWIN_OWNER).await;
    assert_eq!(
        st, 503,
        "a dependency that is down and may come back is not the same class \
         as the deployment being broken: {body}"
    );
    assert_eq!(
        body["error"]["code"], "space_unavailable",
        "the wire inventory has a name for this, not `internal`: {body}"
    );

    // The same failing resolve that answered 503 is the one write: no
    // caller has to ask for that separately.
    let quarantined = h.spaces.quarantined();
    assert_eq!(
        quarantined
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>(),
        [TWIN],
        "the failing resolve registers the tenant: {quarantined:?}"
    );

    let (st, ready) = get_json(&h.app, "/health/ready").await;
    assert_eq!(st, 200, "one closed tenant is not an outage: {ready}");
    let held: Vec<&str> = ready["quarantined"]
        .as_array()
        .map(|rows| rows.iter().filter_map(|r| r["space"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(held, [TWIN], "readiness names the same tenant: {ready}");

    // Another tenant on a reachable database is untouched — one target
    // down must not take the deployment with it.
    let (st, body) = get_in(&h.app, "/op/std.docs.list", HERE, OWNER).await;
    assert_eq!(st, 200, "{body}");
}

/// **A recovering target lifts its own quarantine, with no restart** (18d
/// R-F1). `resolve`'s failing path writes the register and its passing path
/// calls `lift_quarantine` unconditionally, which is what makes the register
/// self-healing for every reason it can hold rather than the rewind alone:
/// a cluster that comes back lifts its own tenants on their next request.
///
/// The target starts absent rather than merely unlistened-to, so that
/// "recovering" means something an operator actually does: creating the
/// database the attachment already names.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovering_target_lifts_its_own_quarantine() {
    const TARGET_DB: &str = "ordo_host_spaces_t5_recovering_t";
    let drop_target = || {
        let admin = Pg {
            db: "postgres".into(),
            ..Pg::default()
        };
        let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {TARGET_DB} WITH (FORCE)"));
    };
    drop_target();

    let env = ordo_exec::PoolConfig::from_env();
    let h = harness_with("ordo_host_spaces_t5_recovering", |pool| {
        Some(Arc::new(ordo_exec::route::AttachedPools::new(
            pool.clone(),
            pool.clone(),
            ordo_exec::PoolConfig::from_env(),
            Arc::new(ordo_exec::route::Installed::new().as_configured("t5-recovering")),
        )) as Arc<dyn PoolResolver>)
    })
    .await;
    ordo_exec::spacedb::ensure_table(&h.pool)
        .await
        .expect("the attachment table");
    ordo_exec::spacedb::attach(
        &h.pool,
        &SpaceId(TWIN.into()),
        &ordo_exec::spacedb::DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: TARGET_DB.into(),
            credential: "t5-recovering".into(),
            custody: ordo_exec::spacedb::DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: TWIN_OWNER.into(),
        },
    )
    .await
    .expect("attached");

    // 1 — down: the database the attachment names does not exist, so
    // resolve refuses and the register notes it, exactly as an ordinary
    // request would.
    let Err(refused) = h.spaces.resolve(TWIN).await else {
        panic!("a database that does not exist resolved");
    };
    assert!(
        matches!(refused, ordo_host::spaces::SpaceRefusal::Unroutable { .. }),
        "{refused}"
    );
    assert_eq!(
        h.spaces
            .quarantined()
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>(),
        [TWIN],
        "the down target's tenant is registered"
    );

    // 2 — recover it for real: create the database the attachment already
    // names. Waited past `route.rs`'s own dial backoff, so the next resolve
    // actually dials rather than being refused by the timer without trying.
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    admin
        .exec(&format!("CREATE DATABASE {TARGET_DB}"))
        .expect("recreated");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    h.spaces
        .resolve(TWIN)
        .await
        .expect("the recovered target resolves, with no restart");
    assert!(
        h.spaces.quarantined().is_empty(),
        "a passing resolve lifts the register: {:?}",
        h.spaces.quarantined()
    );

    let (_, ready) = get_json(&h.app, "/health/ready").await;
    assert_eq!(
        ready["quarantined"].as_array().map(Vec::len),
        Some(0),
        "and readiness agrees, with no restart in between: {ready}"
    );

    drop_target();
}

/// **A dial error is never returned to the caller** (§9.1 R-C4.4). The
/// caller's message is a fixed, opaque sentence regardless of what the
/// dial actually said — never the host, the port, the user or the database
/// name a real Postgres error names, because that is deployment topology a
/// space owner has no business reading, and for a tenant on their own
/// cluster it would be their own infrastructure described back to them by
/// a third party.
///
/// The database is dropped rather than merely unreachable, so the
/// underlying refusal genuinely contains something worth hiding: Postgres's
/// own `FATAL: database "…" does not exist` names the database outright,
/// which is the sentence this test proves never reaches the wire.
#[tokio::test(flavor = "multi_thread")]
async fn a_dial_error_is_never_returned_to_the_caller() {
    const TARGET_DB: &str = "ordo_host_spaces_t5_dial_probe_absent";
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {TARGET_DB} WITH (FORCE)"));

    let env = ordo_exec::PoolConfig::from_env();
    let h = harness_with("ordo_host_spaces_t5_dial_opaque", |pool| {
        Some(Arc::new(ordo_exec::route::AttachedPools::new(
            pool.clone(),
            pool.clone(),
            ordo_exec::PoolConfig::from_env(),
            Arc::new(ordo_exec::route::Installed::new().as_configured("t5-dial-probe")),
        )) as Arc<dyn PoolResolver>)
    })
    .await;
    ordo_exec::spacedb::ensure_table(&h.pool)
        .await
        .expect("the attachment table");
    ordo_exec::spacedb::attach(
        &h.pool,
        &SpaceId(TWIN.into()),
        &ordo_exec::spacedb::DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: TARGET_DB.into(),
            credential: "t5-dial-probe".into(),
            custody: ordo_exec::spacedb::DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: TWIN_OWNER.into(),
        },
    )
    .await
    .expect("attached");

    // The raw shape a caller must never see, read directly at the layer
    // under the door, where the dial error actually lives.
    let Err(refused) = h.spaces.resolve(TWIN).await else {
        panic!("the absent database resolved");
    };
    let raw = refused.to_string();
    assert!(
        raw.contains(TARGET_DB),
        "sanity: the underlying refusal must actually name the database, \
         or this test proves nothing: {raw}"
    );

    let (st, body) = get_in(&h.app, "/op/std.docs.list", TWIN, TWIN_OWNER).await;
    assert_eq!(st, 503, "{body}");
    let text = body.to_string();
    for needle in [
        env.host.as_str(),
        &env.port.to_string(),
        env.user.as_str(),
        TARGET_DB,
        raw.as_str(),
    ] {
        assert!(
            !needle.is_empty() && !text.contains(needle),
            "the caller's body must not carry deployment topology (`{needle}`): {text}"
        );
    }

    let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {TARGET_DB} WITH (FORCE)"));
}
