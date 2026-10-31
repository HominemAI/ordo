//! The exit ops through the real door: `std.space.export` and
//! `std.space.delete`.
//!
//! `ordo-exec`'s suites prove the engines — that a bundle is complete,
//! deterministic and self-loading, and that an erasure makes a surviving
//! envelope unopenable. This file proves the three things §8 pins that
//! neither engine can know about: **owner-only**, **step-up gated**, and
//! **recorded**.
//!
//! # Why the refusals get most of the tests
//!
//! One of these ops discloses an entire space including every sealed field,
//! and the other ends one irreversibly. There is no third op in this system
//! where being wrong about *who may* is this expensive. A test that only
//! proved the owner can export would pass for a build where the manager,
//! the editor and a delegated agent could too.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::cadence::LogPolicy;
use ordo_exec::registry;
use ordo_host::exit_ops::{ExitOps, DELETE, EXPORT};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps, CALLERS_OF};
use ordo_host::{FixtureExecutor, DEFAULT_SPACE};
use ordo_identity::principal::{GrantId, PrincipalId, SpaceId as ISpace, Timestamp as IStamp};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::Value;
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"exit-ops-secret";

/// The fixture's seeded principals (`fixtures/seed.sql`).
const OWNER: &str = "mem_o";
const MANAGER: &str = "mem_1";
const EDITOR: &str = "mem_2";
const VIEWER: &str = "mem_4";

struct Harness {
    state: ApiState,
    stream: Arc<ordo_secevent::Recorder>,
    engine: Arc<AccessEngine>,
    deployment: Arc<ordo_host::deploy::Deployment>,
    bundles: std::path::PathBuf,
    pool: ordo_exec::Pool,
    pg: Pg,
}

async fn harness(db: &str) -> Harness {
    harness_with_blobs(db, None).await
}

/// The same deployment, with a blob tier wired.
///
/// A separate constructor rather than a parameter on every call site,
/// because `blobs: None` is the honest default for the fixture — it has no
/// blob fields — and the one test that needs objects is the one that
/// proves erasing a space takes them with it.
async fn harness_with_blobs(db: &str, blob_dir: Option<&std::path::Path>) -> Harness {
    harness_full(db, blob_dir, false).await
}

/// Either of the above, and with the E7 identity settlement wired when
/// `identities` is set.
async fn harness_full(db: &str, blob_dir: Option<&std::path::Path>, identities: bool) -> Harness {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");

    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");
    ordo_exec::prune::ensure_tables(&pool)
        .await
        .expect("consumer cursors");
    let _ = pool.rows("DELETE FROM \"$capabilitygrant\"").await;

    // The exit ops read the **deployed artifact**, so the test deploys one
    // rather than borrowing the serving fixture's schema. That is the whole
    // point of `ExitOps::deployed`: the bundle's `schema.sql` and the
    // columns the export reads must come from the same stored bytes.
    registry::ensure_tables(&pool).await.expect("registry");
    let _ = pool.rows("DELETE FROM \"$activations\"").await;
    let _ = pool.rows("DELETE FROM \"$artifacts\"").await;
    registry::store(
        &pool,
        &registry::Artifact {
            version: "v_exit".into(),
            artifact: "{\"stand-in\": true}\n".into(),
            source: ordo_conformance::FIXTURE.to_string(),
            ddl: f.ddl.clone(),
            applied_at: ordo_commandlog::Timestamp(NOW),
            actor: "test".into(),
        },
    )
    .await
    .expect("store");
    registry::activate(&pool, "v_exit", "test", ordo_commandlog::Timestamp(NOW))
        .await
        .expect("activate");

    let engine = Arc::new(ordo_host::deployment_engine(&f, &pg).expect("engine"));
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "exit-ops-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("the fixture compiles no `std.space.*` name");

    let bundles = std::env::temp_dir().join(format!("ordo-exp-bundles-{db}"));
    let _ = std::fs::remove_dir_all(&bundles);
    std::fs::create_dir_all(&bundles).expect("bundle root");

    let stream = Arc::new(ordo_secevent::Recorder::default());
    #[allow(clippy::type_complexity)]
    let blob_tier: Option<(
        Arc<ordo_blob::BlobPipeline>,
        Arc<dyn ordo_blob::StoreResolver>,
    )> = blob_dir.map(|dir| {
        let store = ordo_blob::FsStore::shared(dir).expect("blob store");
        let resolver: Arc<dyn ordo_blob::StoreResolver> =
            Arc::new(ordo_blob::SingleStore::new(store));
        let pipe = Arc::new(ordo_blob::BlobPipeline::new(
            Arc::clone(&resolver),
            f.keys.clone(),
            ordo_blob::BlobPolicy::DEFAULT,
        ));
        (pipe, resolver)
    });
    let mut ops = PlatformOps::new(engine.clone(), grants.clone(), Arc::new(|| NOW)).with_exit(
        Arc::new(ExitOps {
            pool: pool.clone(),
            pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
            keys: f.keys.clone(),
            policy: LogPolicy::DEFAULT,
            bundles: bundles.clone(),
            // This harness exercises the way out. `std.space.import`
            // has its own (`tests/import_op.rs`), because what it needs
            // is a staged bundle and a second space to load it into.
            imports: None,
            door: None,
            stream: Some(stream.clone()),
            // `None` unless a test asked for one. The manifest states
            // that as a fact rather than leaving an absent `blobs/`
            // directory to be interpreted — and the deletion skips the
            // object sweep for the same reason: there are no objects,
            // not "we chose not to look".
            blobs: blob_tier.as_ref().map(|(p, _)| Arc::clone(p)),
            stores: blob_tier.as_ref().map(|(_, s)| Arc::clone(s)),
            now: Arc::new(|| NOW),
        }),
    );
    if identities {
        ordo_host::principal_directory::ensure_table(&pool)
            .await
            .expect("directory");
        ordo_host::credential_ops::ensure_table(&pool)
            .await
            .expect("managed");
        ops = ops.with_identities(Arc::new(ordo_host::tenant_identities::TenantIdentities {
            control: pool.clone(),
            connections: Arc::new(ordo_host::tenant_identities::NoConnections),
            revocations: None,
            session_max_secs: 3_600,
            stream: Some(stream.clone()),
            now: Arc::new(|| NOW),
        }));
    }
    let ops = Arc::new(ops);

    let access = Arc::new(ordo_host::IdentityAccess::sharing(
        engine.clone(),
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    ));

    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"exit-ops-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        stream,
        engine,
        deployment,
        bundles,
        pool,
        pg,
    }
}

fn token(principal: &str, present: Option<u64>) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: present.map(IStamp),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// The strongest capability token that can exist for an op: it names the op
/// and it claims fresh presence. It must still be refused — §5 pins that a
/// capability token can never satisfy step-up, which is the mechanism that
/// makes these two structurally undelegable. An owner may delegate the
/// owner's authority over data; ending the space is not that.
fn capability_token(engine: &AccessEngine, principal: &str, op: &str) -> String {
    let set: BTreeSet<String> = BTreeSet::from([op.to_string()]);
    let grant = engine
        .grant_for(
            GrantId("gr_exit".into()),
            &ISpace(DEFAULT_SPACE.to_string()),
            set,
            IStamp(NOW + 3_600),
        )
        .expect("the op is deployed");
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: Some(IStamp(NOW)),
            capability: Some(grant),
            delegation: None,
            session: None,
        },
    )
}

async fn call(
    state: &ApiState,
    method: Method,
    path: &str,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    call_with(state, method, path, bearer, None).await
}

/// `body` is the POST envelope. A `Write` op's parameters ride the body,
/// never the query string — `split_envelope` reads one and `split_query`
/// the other, and which applies is decided by the method the op's `OpKind`
/// pinned.
async fn call_with(
    state: &ApiState,
    method: Method,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-ordo-space", DEFAULT_SPACE);
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let payload = match &body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = router(state.clone())
        .oneshot(req.body(payload).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// **Ciphertext mode throughout this file, and not for convenience.**
///
/// `fixtures/seed.sql` writes `'\x00'::bytea` into every sealed column —
/// a one-byte placeholder, not an envelope, left from before anything in
/// the tree could encrypt. An export promising plaintext therefore refuses
/// against the conformance fixture, *correctly*: it is a value the platform
/// holds and cannot open, and handing over a null would be the silent
/// under-delivery the refusal exists to prevent.
///
/// So the door tests take the honest mode for the state they are in.
/// Opened mode is proven end to end in `ordo-exec`'s `tests/export.rs`,
/// against rows whose envelopes are real.
async fn export(state: &ApiState, bearer: Option<&str>) -> (StatusCode, Value) {
    call_with(
        state,
        Method::POST,
        &format!("/op/{EXPORT}"),
        bearer,
        Some(serde_json::json!({ "ciphertext": true })),
    )
    .await
}

async fn delete(state: &ApiState, confirm: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    call(
        state,
        Method::DELETE,
        &format!("/op/{DELETE}?confirm={confirm}"),
        bearer,
    )
    .await
}

fn class(body: &Value) -> String {
    body["error"]["code"]
        .as_str()
        .unwrap_or("<none>")
        .to_string()
}

// ── who may ──────────────────────────────────────────────────────────────

/// **Owner only, and every other answer is the same answer.**
///
/// The baseline is a name nobody registered. A manager, an editor and a
/// viewer must be indistinguishable from that — a refusal that told a
/// manager *"this op exists and you may not call it"* would confirm the
/// deployment serves exit ops, which is exactly the fact a prober wants.
/// The positive control is in the same test so the file cannot pass by the
/// op never working at all.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_reaches_either_exit_op_and_every_refusal_is_one_refusal() {
    let h = harness("ordo_exp_door_who").await;

    let baseline = call(
        &h.state,
        Method::POST,
        "/op/std.space.noSuchPlatformOp",
        Some(&token(OWNER, Some(NOW))),
    )
    .await;
    assert_eq!(baseline.0, StatusCode::FORBIDDEN);

    for op in [EXPORT, DELETE] {
        for who in [MANAGER, EDITOR, VIEWER] {
            let got = if op == EXPORT {
                export(&h.state, Some(&token(who, Some(NOW)))).await
            } else {
                delete(&h.state, DEFAULT_SPACE, Some(&token(who, Some(NOW)))).await
            };
            assert_eq!(
                (got.0, class(&got.1)),
                (baseline.0, class(&baseline.1)),
                "`{who}` got a different answer for `{op}` than for an op nobody \
                 registered, which tells a prober the deployment serves exit ops"
            );
        }
        // An owner with no recent presence assertion: a perfectly valid
        // session that has not touched a passkey. Same refusal.
        let stale = if op == EXPORT {
            export(&h.state, Some(&token(OWNER, None))).await
        } else {
            delete(&h.state, DEFAULT_SPACE, Some(&token(OWNER, None))).await
        };
        assert_eq!((stale.0, class(&stale.1)), (baseline.0, class(&baseline.1)));

        // The strongest capability token there is. Refused, and that is
        // what makes these ops human-only without a second mechanism.
        let delegated = capability_token(&h.engine, OWNER, op);
        let got = if op == EXPORT {
            export(&h.state, Some(&delegated)).await
        } else {
            delete(&h.state, DEFAULT_SPACE, Some(&delegated)).await
        };
        assert_eq!(
            (got.0, class(&got.1)),
            (baseline.0, class(&baseline.1)),
            "`{op}` was reachable through a capability token, so an agent holding a \
             delegation could end or disclose somebody's whole space"
        );
    }

    // The positive control.
    let (status, body) = export(&h.state, Some(&token(OWNER, Some(NOW)))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The catalog scopes like the door does: an owner-only op is **omitted**
/// from a manager's catalog rather than listed as forbidden, and omitted
/// from the owner's own when their step-up has gone stale. A
/// listed-but-forbidden entry would tell the asker the op exists, which is
/// the fact the uniform refusal above is protecting.
#[tokio::test(flavor = "multi_thread")]
async fn the_exit_ops_appear_only_in_a_fresh_owners_catalog() {
    let h = harness("ordo_exp_door_catalog").await;
    let visible = |bearer: &str| -> Vec<String> {
        h.deployment
            .manifests()
            .into_iter()
            .filter(|(name, _)| {
                matches!(
                    ordo_api::AccessChecker::check(
                        h.state.access.as_ref(),
                        bearer,
                        DEFAULT_SPACE,
                        name
                    ),
                    ordo_api::AccessDecision::Allow { .. }
                )
            })
            .map(|(name, _)| name)
            .collect()
    };
    let has = |v: &[String], n: &str| v.iter().any(|x| x == n);

    let owner = visible(&token(OWNER, Some(NOW)));
    assert!(
        has(&owner, EXPORT) && has(&owner, DELETE),
        "a fresh owner cannot see the door out: {owner:?}"
    );

    let manager = visible(&token(MANAGER, Some(NOW)));
    assert!(
        !has(&manager, EXPORT) && !has(&manager, DELETE),
        "the exit ops are in a manager's catalog: {manager:?}"
    );
    assert!(
        has(&manager, CALLERS_OF),
        "the manager's own platform op vanished too, so this test is about a broken \
         catalog rather than about scoping: {manager:?}"
    );
    for who in [EDITOR, VIEWER] {
        let v = visible(&token(who, Some(NOW)));
        assert!(!has(&v, EXPORT) && !has(&v, DELETE), "`{who}`: {v:?}");
    }

    // The catalog shows what you may call NOW, and step-up is part of that.
    let stale = visible(&token(OWNER, None));
    assert!(!has(&stale, EXPORT) && !has(&stale, DELETE), "{stale:?}");

    // And both are really in the registry the door dispatches through, so
    // the omissions above are scoping rather than an absent op.
    let all: Vec<String> = h
        .deployment
        .manifests()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert!(has(&all, EXPORT) && has(&all, DELETE), "{all:?}");
}

/// `OpKind` fixes the verb. An export is a `Write` because it produces a
/// durable artifact and a disclosure record, so it is a POST — and a GET
/// against it is refused by method enforcement rather than served.
#[tokio::test(flavor = "multi_thread")]
async fn the_verbs_are_the_ops_kinds() {
    let h = harness("ordo_exp_door_verbs").await;
    let t = token(OWNER, Some(NOW));

    let (status, _) = call(&h.state, Method::GET, &format!("/op/{EXPORT}"), Some(&t)).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _) = call(
        &h.state,
        Method::POST,
        &format!("/op/{DELETE}?confirm={DEFAULT_SPACE}"),
        Some(&t),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

// ── what they do ─────────────────────────────────────────────────────────

/// The export runs end to end through the door and leaves a bundle on
/// disk whose members are the ones the report names.
#[tokio::test(flavor = "multi_thread")]
async fn an_owners_export_writes_a_bundle_and_reports_it() {
    let h = harness("ordo_exp_door_export").await;
    let (status, body) = export(&h.state, Some(&token(OWNER, Some(NOW)))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["space"], DEFAULT_SPACE);
    assert_eq!(row["version"], "v_exit");
    assert_eq!(row["sealed"], "ciphertext");

    let dir = std::path::Path::new(row["bundle"].as_str().expect("a bundle path"));
    for member in row["members"].as_array().expect("members") {
        let name = member.as_str().expect("a name");
        assert!(
            dir.join(name).exists(),
            "the report names `{name}` and the bundle does not have it"
        );
    }
    assert!(dir.join("schema.sql").exists() && dir.join("manifest.json").exists());
    let schema = std::fs::read_to_string(dir.join("schema.sql")).expect("readable");
    assert!(
        schema.contains("CREATE TABLE \"orders\""),
        "the bundle's schema.sql is not the deployed DDL"
    );

    // A second export never overwrites the first. The bundles are
    // byte-identical, which is what makes overwriting look harmless — and
    // the one time it is not is the time somebody is still reading the
    // older one.
    let (_, second) = export(&h.state, Some(&token(OWNER, Some(NOW)))).await;
    let other = second["rows"][0]["bundle"].as_str().expect("a path");
    assert_ne!(other, row["bundle"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(dir.join("data/orders.jsonl")).expect("first"),
        std::fs::read_to_string(std::path::Path::new(other).join("data/orders.jsonl"))
            .expect("second"),
        "two exports of one unchanged space differ"
    );
    assert!(h.bundles.join(DEFAULT_SPACE).join("0001").exists());

    // The one knob is a bool, so a typo is refused by the DOOR, in the
    // door's own vocabulary, before the op is reached. That is why it is a
    // bool: a `mode: text` would put the enum check inside the handler,
    // where `invalid_param` is not a word the executor may say, and the
    // typo would have reached the caller as `internal`.
    let (status, body) = call_with(
        &h.state,
        Method::POST,
        &format!("/op/{EXPORT}"),
        Some(&token(OWNER, Some(NOW))),
        Some(serde_json::json!({ "ciphertext": "whatever" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(class(&body), "invalid_param");

    // And an unknown knob is refused too, rather than ignored into a
    // default that is *total disclosure*.
    let (status, body) = call_with(
        &h.state,
        Method::POST,
        &format!("/op/{EXPORT}"),
        Some(&token(OWNER, Some(NOW))),
        Some(serde_json::json!({ "mode": "ciphertext" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(class(&body), "unknown_param");
}

/// `plan-production-ops.md` R1a: the export's response names the bundle by
/// one digest, `sha256(manifest.json)`, so a restore marker recorded later
/// (`RestoreSource::Bundle { manifest_sha256 }`) can point at exactly this
/// bundle with no platform needed to check it.
#[tokio::test(flavor = "multi_thread")]
async fn an_owners_export_reports_the_bundle_digest() {
    let h = harness("ordo_exp_door_export_digest").await;
    let (status, body) = export(&h.state, Some(&token(OWNER, Some(NOW)))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];

    let dir = std::path::Path::new(row["bundle"].as_str().expect("a bundle path"));
    let manifest_bytes = std::fs::read(dir.join("manifest.json")).expect("a manifest on disk");
    let expected = ordo_exec::export::bundle_digest(&manifest_bytes);

    let reported = row["manifest_sha256"]
        .as_str()
        .expect("the response names the bundle digest");
    assert_eq!(reported.len(), 64, "sha256 is 32 bytes of hex");
    assert!(reported.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(
        reported, expected,
        "the reported digest must be the manifest's own"
    );
}

/// The confirmation, at the door. And then the deletion, which the
/// engine's own suite has already proven and this one only has to see
/// arrive.
#[tokio::test(flavor = "multi_thread")]
async fn deletion_needs_the_spaces_own_id_and_then_ends_the_space() {
    let h = harness("ordo_exp_door_delete").await;
    let t = token(OWNER, Some(NOW));

    // Absent: the door's own type check, before the op is reached.
    let (status, body) = call(&h.state, Method::DELETE, &format!("/op/{DELETE}"), Some(&t)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(class(&body), "missing_param");

    // Present, bound, and wrong: the op's refusal, and deliberately not the
    // door's. A caller told "wrong type" would go looking for the wrong bug.
    let (status, body) = delete(&h.state, "yes", Some(&t)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(class(&body), "not_confirmed");
    let before: i64 =
        h.pg.query("SELECT count(*) FROM \"orders\"")
            .expect("count")[0][0]
            .as_deref()
            .and_then(|s| s.parse().ok())
            .expect("a number");
    assert!(
        before > 0,
        "the fixture seeded nothing, so this proves little"
    );

    // Right.
    let (status, body) = delete(&h.state, DEFAULT_SPACE, Some(&t)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["space"], DEFAULT_SPACE);
    assert!(row["rowsDeleted"].as_i64().expect("a count") >= before);
    assert_eq!(
        row["retained"],
        serde_json::json!(ordo_exec::erase::RETAINED),
        "the answer states the residue rather than leaving it to a document"
    );
    let after: i64 =
        h.pg.query(&format!(
            "SELECT count(*) FROM \"orders\" WHERE \"$space\" = '{DEFAULT_SPACE}'"
        ))
        .expect("count")[0][0]
            .as_deref()
            .and_then(|s| s.parse().ok())
            .expect("a number");
    assert_eq!(after, 0);
    // The artifact registry is deployment state, not the space's. A
    // deleted space must not take the deployment's program with it.
    assert!(registry::active(&h.pool).await.expect("registry").is_some());
}

/// A deployment that registers an exit op and wires no state for it is a
/// broken deployment, and says so rather than answering an owner's export
/// with an empty bundle.
#[tokio::test(flavor = "multi_thread")]
async fn an_unwired_exit_op_refuses_rather_than_answering_partially() {
    let h = harness("ordo_exp_door_unwired").await;
    // Rebuild the state with the exit ops registered and unwired.
    let f = Arc::new(Fixture::load().expect("fixture"));
    let engine = h.engine.clone();
    let grants = Arc::new(PgGrantRegistry::new(h.pool.clone()));
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "exit-ops-unwired",
        f.clone(),
        ordo_exec::Executor {
            pool: h.pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("specs");
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: h.pg.clone(),
                page_size: 50,
            }),
            // No `with_exit`.
            Arc::new(PlatformOps::new(engine, grants, Arc::new(|| NOW))),
        )),
        ..h.state.clone()
    };

    let (status, body) = export(&state, Some(&token(OWNER, Some(NOW)))).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(class(&body), "internal");
}

// ── the record ───────────────────────────────────────────────────────────

/// The third thing §8 pins, and the one this file exists to prove because
/// no engine can: **the owner cannot do either of these without leaving a
/// record.**
///
/// The record is a `CustodyAction` in the **security-event stream** rather
/// than a group in the command log, and `exit_ops`'s module doc argues
/// why: the command log records writes to entity rows, export mutates no
/// row, and deletion destroys the chain. The stream is where epoch
/// rotation and epoch destruction already go.
///
/// Space-scoped for both, which for the deletion works only because
/// `erase::RETAINED` keeps that stream — the one record that must outlive
/// a space is the record that it was deleted.
#[tokio::test(flavor = "multi_thread")]
async fn both_exit_ops_leave_a_custody_record_in_the_owners_own_stream() {
    use ordo_secevent::{EventKind, Scope};

    let h = harness("ordo_exp_door_record").await;
    let t = token(OWNER, Some(NOW));

    // A refusal discloses nothing, and must record nothing — a false
    // total-disclosure event in the owner's own stream would be worse than
    // no event at all.
    let (status, _) = export(&h.state, Some(&token(MANAGER, Some(NOW)))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = delete(&h.state, "not-the-space", Some(&t)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        h.stream
            .drain_all(ordo_commandlog::Timestamp(NOW))
            .is_empty(),
        "a refused exit op recorded a custody action"
    );

    let (status, body) = export(&h.state, Some(&t)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = delete(&h.state, DEFAULT_SPACE, Some(&t)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let entries = h.stream.drain_all(ordo_commandlog::Timestamp(NOW));
    let actions: Vec<(String, String, String)> = entries
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::CustodyAction { action } => Some((
                action.clone(),
                e.op.clone(),
                match &e.scope {
                    Scope::Space(s) => s.0.clone(),
                    Scope::Deployment => "<deployment>".into(),
                },
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        actions,
        vec![
            (
                "space_exported".to_string(),
                EXPORT.to_string(),
                DEFAULT_SPACE.to_string()
            ),
            (
                "space_erased".to_string(),
                DELETE.to_string(),
                DEFAULT_SPACE.to_string()
            ),
        ],
        "the two acts a space owner cannot take silently: {entries:?}"
    );

    // The subject is the owner, rendered in the same actor taxonomy a
    // command group is attributed with — so the two records join on
    // attribution without a translation table.
    for e in &entries {
        assert_eq!(e.subject.render(), format!("user:{OWNER}"));
    }
}

/// **Erasing a space takes its files with it**, and this test exists
/// because the first version of the op did not.
///
/// `ExitOps::stores` was declared, doc-commented with the reason, and then
/// never read — so `std.space.delete` destroyed the keys, deleted the rows,
/// and left every uploaded object in the bucket forever.
///
/// Crypto-erasure alone looks like enough and is not. The DEK is gone, so
/// nothing can open those objects again — but an object's **existence and
/// its size are observable without any key**, which is precisely what
/// `blobgc::erase_space`'s doc says erasure must not leave behind. A space
/// that was erased should not still be listable, and an operator should not
/// still be paying for it.
#[tokio::test(flavor = "multi_thread")]
async fn erasing_a_space_deletes_its_objects_too() {
    let dir = std::env::temp_dir().join(format!("ordo-exit-blobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("blob root");

    let h = harness_with_blobs("ordo_exp_door_blobs", Some(&dir)).await;
    let space = ordo_crypto::SpaceId(DEFAULT_SPACE.to_string());
    let keys = ordo_conformance::Fixture::load().expect("fixture").keys;

    // Two objects, stored the way an upload stores them: sealed through the
    // pipeline, with a `$blobref` row naming each. The entity and field are
    // the fixture's own — what matters here is the reference, not the
    // column, because erasure walks `$blobref` rather than the schema.
    let store = ordo_blob::FsStore::shared(&dir).expect("store");
    let pipe = ordo_blob::BlobPipeline::new(
        std::sync::Arc::new(ordo_blob::SingleStore::new(std::sync::Arc::clone(&store))),
        keys.clone(),
        ordo_blob::BlobPolicy::DEFAULT,
    );
    let mut handles = Vec::new();
    for (i, bytes) in [b"the signed contract".as_slice(), b"an identity photo"]
        .iter()
        .enumerate()
    {
        let stored = pipe
            .put(
                &ordo_blob::Upload {
                    space: &space,
                    entity: "docs",
                    field: "scan",
                    mime: "application/pdf",
                    plain_len: bytes.len() as u64,
                    correlation: "test",
                    intent: None,
                },
                ordo_blob::once(bytes.to_vec()),
            )
            .await
            .expect("put");
        let tx = h.pool.begin().await.expect("tx");
        h.pool
            .rows(&ordo_crypto::blobref::insert_sql(
                &space,
                stored.handle(),
                stored.epoch(),
                "docs",
                "scan",
                stored.mime(),
                stored.size(),
                stored.integrity(),
                stored.bytes(),
                NOW as i64,
                "",
            ))
            .await
            .expect("reference");
        tx.rollback().await.ok();
        handles.push(*stored.handle());
        let _ = i;
    }

    for handle in &handles {
        assert!(
            store.head(&space, handle).await.is_ok(),
            "the object should be there before the erasure"
        );
    }

    let (status, body) = delete(&h.state, DEFAULT_SPACE, Some(&token(OWNER, Some(NOW)))).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The count is reported rather than silent: an owner ending a space is
    // owed the number of files that went with it.
    assert_eq!(body["rows"][0]["objectsDeleted"], 2, "{body}");

    for handle in &handles {
        assert!(
            store.head(&space, handle).await.is_err(),
            "an erased space's objects are still in the bucket — their size and \
             existence readable with no key at all"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// **D-S5 through the door.** A tenant with one file in the deployment's
/// store and one in a bucket it owns: the first is deleted, the second is
/// left, and the answer and the tenant's security stream both say what was
/// left. The entry names the store, the count and a digest, and carries no
/// credential.
///
/// The harness resolves every name to one filesystem store, so the object
/// being still there after the erase can only mean erasure chose to leave
/// it.
#[tokio::test(flavor = "multi_thread")]
async fn erasing_a_space_leaves_files_in_its_own_bucket_and_records_them() {
    use ordo_secevent::EventKind;

    let dir = std::env::temp_dir().join(format!("ordo-exit-left-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("blob root");

    let h = harness_with_blobs("ordo_exp_door_left", Some(&dir)).await;
    let space = ordo_crypto::SpaceId(DEFAULT_SPACE.to_string());
    let keys = ordo_conformance::Fixture::load().expect("fixture").keys;
    let store = ordo_blob::FsStore::shared(&dir).expect("store");
    let pipe = ordo_blob::BlobPipeline::new(
        std::sync::Arc::new(ordo_blob::SingleStore::new(std::sync::Arc::clone(&store))),
        keys.clone(),
        ordo_blob::BlobPolicy::DEFAULT,
    );
    let own_bucket = ordo_exec::bucket::Attachment {
        endpoint: "https://s3.tenant.example".into(),
        region: "eu-west-1".into(),
        bucket: "acme-files".into(),
        credential: "acme-cred-label".into(),
        custody: ordo_exec::bucket::BucketCustody::OwnPerimeter,
        path_style: None,
        key_prefix: None,
        attached_at: 0,
        attached_by: String::new(),
    }
    .store_name();

    let mut handles = Vec::new();
    for (bytes, store_name) in [
        (b"in the deployment's store".as_slice(), ""),
        (b"in the tenant's bucket", own_bucket.as_str()),
    ] {
        let stored = pipe
            .put(
                &ordo_blob::Upload {
                    space: &space,
                    entity: "docs",
                    field: "scan",
                    mime: "application/pdf",
                    plain_len: bytes.len() as u64,
                    correlation: "test",
                    intent: None,
                },
                ordo_blob::once(bytes.to_vec()),
            )
            .await
            .expect("put");
        h.pool
            .rows(&ordo_crypto::blobref::insert_sql(
                &space,
                stored.handle(),
                stored.epoch(),
                "docs",
                "scan",
                stored.mime(),
                stored.size(),
                stored.integrity(),
                stored.bytes(),
                NOW as i64,
                store_name,
            ))
            .await
            .expect("reference");
        handles.push(*stored.handle());
    }

    let (status, body) = delete(&h.state, DEFAULT_SPACE, Some(&token(OWNER, Some(NOW)))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["objectsDeleted"], 1, "{body}");
    let left = row["objectsLeft"].as_array().expect("objectsLeft");
    assert_eq!(left.len(), 1, "{body}");
    assert_eq!(left[0]["count"], 1);
    assert_eq!(left[0]["store"], "https://s3.tenant.example|acme-files|");
    assert!(left[0]["digest"].as_str().unwrap().starts_with("sha256:"));

    assert!(
        store.head(&space, &handles[0]).await.is_err(),
        "the deployment's own file is deleted"
    );
    assert!(
        store.head(&space, &handles[1]).await.is_ok(),
        "a file in a bucket the tenant owns is left"
    );

    let actions: Vec<String> = h
        .stream
        .drain_all(ordo_commandlog::Timestamp(NOW))
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::CustodyAction { action } => Some(action.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(actions.len(), 2, "{actions:?}");
    assert_eq!(actions[0], "space_erased");
    assert!(
        actions[1].starts_with("objects_left store=https://s3.tenant.example|acme-files| count=1 "),
        "{actions:?}"
    );
    assert!(actions[1].contains("digest=sha256:"), "{actions:?}");
    assert!(
        !actions[1].contains("acme-cred-label"),
        "no credential label in the record: {actions:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **E7 through the door.** Deleting a tenant with the settlement wired
/// settles the people it made, and the answer says what happened. Without it
/// nothing is touched, which is what a deployment with no `$managed` table
/// gets.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_tenant_settles_the_people_it_made_and_says_so() {
    let h = harness_full("ordo_exp_door_delete_identities", None, true).await;
    for t in ["$principal", "$authenticator", "$managed", "$signedin"] {
        h.pool
            .rows(&format!("DELETE FROM \"{t}\""))
            .await
            .expect("clean");
    }
    for id in ["usr_used", "usr_unused"] {
        h.pool
            .rows(&format!(
                "INSERT INTO \"$principal\" (\"id\", \"kind\", \"display\", \"created_at\") \
                 VALUES ('{id}', 'usr', '{id}', 1)"
            ))
            .await
            .expect("principal");
        h.pool
            .rows(&format!(
                "INSERT INTO \"$managed\" (\"principal\", \"tenant\", \"created_by\", \"created_at\") \
                 VALUES ('{id}', '{DEFAULT_SPACE}', 'mem_o', 1)"
            ))
            .await
            .expect("managed");
    }
    h.pool
        .rows("INSERT INTO \"$signedin\" (\"principal\", \"first_at\") VALUES ('usr_used', 5)")
        .await
        .expect("signed in");

    let (status, body) = delete(&h.state, DEFAULT_SPACE, Some(&token(OWNER, Some(NOW)))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids = &body["rows"][0]["identities"];
    assert_eq!(ids["released"], serde_json::json!(["usr_used"]), "{body}");
    assert_eq!(ids["removed"], serde_json::json!(["usr_unused"]), "{body}");
    let left = h
        .pool
        .rows("SELECT \"id\" FROM \"$principal\" ORDER BY \"id\"")
        .await
        .expect("left");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0][0].as_deref(), Some("usr_used"));
}
