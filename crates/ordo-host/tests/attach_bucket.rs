//! `std.space.attachBucket` through the real door.
//!
//! `ordo-exec`'s `tests/bucket.rs` proves the storage layer — that a repeat
//! is a no-op and a different target is a change. This file proves the
//! things no storage layer can know about, which are exactly the three
//! `definition-language.md` §2 pin (1) demands of the op: **space-owner
//! consent**, **step-up gated**, and **recorded**.
//!
//! It is `tests/exit_ops.rs`'s shape deliberately. The two ops are
//! siblings — both decide what leaving the platform means, both are
//! owner-only, both write a `CustodyAction` and no entity row — so a test
//! that proved a different set of properties for one of them would be
//! describing a difference that does not exist.
//!
//! # The path this file exists to cover
//!
//! Until it, `attachBucket` had unit tests on `ordo_exec::bucket` and had
//! **never been called through `PlatformOps::execute`**. An op that is in
//! `specs()` is registered, granted, step-up-pinned and advertised in every
//! catalog; one that had never been dispatched was a surface with a tested
//! basement and an untested front door.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::bucket::{self, BucketCustody};
use ordo_exec::registry;
use ordo_host::bucket_ops::{BucketOps, BucketProbe, OwnStore, ATTACH_BUCKET, DETACH_BUCKET};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::provision::HostAllowlist;
use ordo_host::{FixtureExecutor, DEFAULT_SPACE};
use ordo_identity::principal::{GrantId, PrincipalId, SpaceId as ISpace, Timestamp as IStamp};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"attach-bucket-secret";

const OWNER: &str = "mem_o";
const MANAGER: &str = "mem_1";
const EDITOR: &str = "mem_2";
const VIEWER: &str = "mem_4";

/// The credential label the operator installed. A **label**, never a key —
/// see `ordo_exec::bucket`'s module doc for why a secret here would be a
/// secret in the plaintext command log.
const CREDENTIAL: &str = "acme-prod";

/// The endpoint every test attaches at, and the one the harness puts on the
/// allowlist. Named rather than repeated so a test that means to be off the
/// allowlist cannot be off it by accident.
const ALLOWED_ENDPOINT: &str = "https://s3.eu-west-1.amazonaws.com";

/// The deployment's **own** bucket, behind the same endpoint. Behind the
/// same endpoint deliberately: §10 B-X3.1 refuses the platform's bucket and
/// not the platform's account, so a test whose own-store endpoint differed
/// from the attachment's could pass on the endpoint alone.
const OWN_BUCKET: &str = "platform-files";

/// The credential provider, standing in for `ordo_exec::creds::Dir`.
///
/// Holds one label and answers `Err` for every other, which is the whole of
/// what `BucketOps` asks of it: "not installed", "names a Postgres
/// password" and "not for you" are one answer through this trait by
/// `BucketCredentials`' own documented design.
struct Installed;

impl ordo_exec::creds::BucketCredentials for Installed {
    fn key_pair(&self, label: &str) -> Result<(String, String), String> {
        if label == CREDENTIAL {
            Ok(("AKIAEXAMPLE".into(), "s3cr3t".into()))
        } else {
            Err("no bucket credential under that label".into())
        }
    }
}

/// The probe, with the answer a test wants.
///
/// A double rather than a real `head`, because the probe is the one part of
/// the op that opens a socket: a suite that needed a reachable bucket to
/// reach the refusals would be a suite in which the refusals are not
/// tested. What the double **does** prove is everything about the probe
/// that is this op's rather than S3's — that it runs at all, that it runs
/// before the row is written, and that its two failures answer with the two
/// different classes. `HeadProbe` is what every deployment gets and is
/// asserted separately, as a unit, for the mapping it owns.
struct Probe {
    answer: std::sync::Mutex<Result<(), ordo_blob::BlobError>>,
    /// The configuration the probe was handed, kept so a test can assert
    /// the probe signed for the target the resolver would address.
    seen: std::sync::Mutex<Option<(String, String, Option<String>)>>,
}

impl Probe {
    fn passing() -> Probe {
        Probe {
            answer: std::sync::Mutex::new(Ok(())),
            seen: std::sync::Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl BucketProbe for Probe {
    async fn probe(
        &self,
        cfg: &ordo_blob::S3Config,
        _space: &ordo_crypto::SpaceId,
    ) -> Result<(), ordo_blob::BlobError> {
        *self.seen.lock().unwrap() = Some((
            cfg.endpoint.host.clone(),
            cfg.bucket.clone(),
            cfg.key_prefix.clone(),
        ));
        match &*self.answer.lock().unwrap() {
            Ok(()) => Ok(()),
            Err(e) => Err(match e {
                ordo_blob::BlobError::Refused(m) => ordo_blob::BlobError::Refused(m.clone()),
                ordo_blob::BlobError::Transport(m) => ordo_blob::BlobError::Transport(m.clone()),
                _ => ordo_blob::BlobError::NotFound,
            }),
        }
    }
}

struct Harness {
    state: ApiState,
    stream: Arc<ordo_secevent::Recorder>,
    engine: Arc<AccessEngine>,
    pool: ordo_exec::Pool,
    probe: Arc<Probe>,
}

/// The harness every test uses: the endpoint on the allowlist, the
/// deployment's own bucket known, plaintext refused, and a passing probe.
async fn harness(db: &str) -> Harness {
    harness_with(db, HostAllowlist::of(ALLOWED_ENDPOINT_HOST), false).await
}

const ALLOWED_ENDPOINT_HOST: &str = "s3.eu-west-1.amazonaws.com";

async fn harness_with(db: &str, hosts: HostAllowlist, allow_plaintext: bool) -> Harness {
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
    bucket::ensure_table(&pool).await.expect("attachments");
    let _ = pool.rows("DELETE FROM \"$capabilitygrant\"").await;
    let _ = pool.rows("DELETE FROM \"$spacebucket\"").await;

    registry::ensure_tables(&pool).await.expect("registry");
    let _ = pool.rows("DELETE FROM \"$activations\"").await;
    let _ = pool.rows("DELETE FROM \"$artifacts\"").await;

    let engine = Arc::new(ordo_host::deployment_engine(&f, &pg).expect("engine"));
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "attach-bucket-test",
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

    let stream = Arc::new(ordo_secevent::Recorder::default());
    let probe = Arc::new(Probe::passing());
    let ops = Arc::new(
        PlatformOps::new(engine.clone(), grants.clone(), Arc::new(|| NOW)).with_buckets(Arc::new(
            BucketOps {
                pool: pool.clone(),
                pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
                stream: Some(stream.clone()),
                credentials: Arc::new(Installed),
                hosts,
                own_store: Some(OwnStore::at_endpoint(ALLOWED_ENDPOINT, OWN_BUCKET)),
                allow_plaintext,
                probe: probe.clone(),
                now: Arc::new(|| NOW),
            },
        )),
    );

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
        page_key: PageKey::from_secret(b"attach-bucket-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        stream,
        engine,
        pool,
        probe,
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

/// The strongest capability token that can exist for this op: it names the
/// op and claims fresh presence. It must still be refused — §5 pins that a
/// capability token can never satisfy step-up, which is what makes storage
/// custody structurally undelegable. An owner may delegate authority over
/// data; deciding who can read the files is not that.
fn capability_token(engine: &AccessEngine, principal: &str) -> String {
    let set: BTreeSet<String> = BTreeSet::from([ATTACH_BUCKET.to_string()]);
    let grant = engine
        .grant_for(
            GrantId("gr_bucket".into()),
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

fn body(bucket_name: &str, platform_sealed: bool) -> Value {
    json!({
        "endpoint": ALLOWED_ENDPOINT,
        "region": "eu-west-1",
        "bucket": bucket_name,
        "credential": CREDENTIAL,
        "platformSealed": platform_sealed,
    })
}

async fn attach(state: &ApiState, bearer: Option<&str>, payload: Value) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(format!("/op/{ATTACH_BUCKET}"))
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = router(state.clone())
        .oneshot(req.body(Body::from(payload.to_string())).unwrap())
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

fn class(body: &Value) -> String {
    body["error"]["code"]
        .as_str()
        .unwrap_or("<none>")
        .to_string()
}

// ── who may ──────────────────────────────────────────────────────────────

/// **Owner only, and every other answer is the same answer.** A manager
/// administers who may do what; deciding where a space's files live and who
/// can read them decides what leaving the platform means, and that is not
/// administration.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_may_attach_a_bucket() {
    let h = harness("ordo_attach_who").await;

    for who in [MANAGER, EDITOR, VIEWER] {
        let (status, b) = attach(
            &h.state,
            Some(&token(who, Some(NOW))),
            body("acme-files", true),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who} reached it: {b}");
        assert_eq!(class(&b), "access_denied", "{who}");
    }

    // And nothing was stored by any of them.
    assert!(
        bucket::lookup(&h.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
            .await
            .unwrap()
            .is_none()
    );

    let (status, b) = attach(
        &h.state,
        Some(&token(OWNER, Some(NOW))),
        body("acme-files", true),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{b}");
}

/// **Step-up, and a capability token can never satisfy it.** The
/// registration loop pins presence on every platform op, and §5 makes that
/// the platform's one mechanism for *not delegable* — reused here rather
/// than minting a second flag that would be a second thing to get right.
#[tokio::test(flavor = "multi_thread")]
async fn presence_is_required_and_a_capability_cannot_stand_in_for_it() {
    let h = harness("ordo_attach_stepup").await;

    // The owner, without presence.
    let (status, b) = attach(
        &h.state,
        Some(&token(OWNER, None)),
        body("acme-files", true),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "a stale token attached a bucket: {b}"
    );

    // The owner, with a capability token naming this very op AND fresh
    // presence. Still refused.
    let cap = capability_token(&h.engine, OWNER);
    let (status, b) = attach(&h.state, Some(&cap), body("acme-files", true)).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "a capability token satisfied step-up, which §5 pins it can never do: {b}"
    );

    // No token at all.
    let (status, _) = attach(&h.state, None, body("acme-files", true)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    assert!(
        bucket::lookup(&h.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
            .await
            .unwrap()
            .is_none()
    );
}

// ── what it does ─────────────────────────────────────────────────────────

/// The happy path, and the response's own disclosure.
#[tokio::test(flavor = "multi_thread")]
async fn an_attachment_lands_and_states_what_erasure_will_mean() {
    let h = harness("ordo_attach_ok").await;
    let owner = token(OWNER, Some(NOW));

    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    let row = &b["rows"][0];
    assert_eq!(row["bucket"], "acme-files");
    assert_eq!(row["custody"], "platform-sealed");
    assert_eq!(row["outcome"], "attached");
    // The consent sentence comes from the platform, not from a renderer —
    // a disclosure assembled by the UI is a disclosure that differs per UI.
    assert!(
        row["disclosure"]
            .as_str()
            .unwrap_or_default()
            .contains("permanently unreadable"),
        "{row}"
    );
    // And it says what does NOT happen, which is the half an operator
    // otherwise discovers six months later. Since 2026-10-06
    // (`st-attach-existing` ask 2) files already written stay in the bucket
    // they were written to, and the sentence says so.
    assert!(
        row["existingObjects"]
            .as_str()
            .unwrap_or_default()
            .contains("stay in the bucket they were written to"),
        "{row}"
    );
    // **§10 B-X5: occupancy is the platform's to record and expose.** One,
    // because D-D refuses a second tenant — and the number is returned
    // rather than assumed by the caller.
    assert_eq!(row["occupancy"], json!(1), "{row}");

    let stored = bucket::lookup(&h.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(stored.bucket, "acme-files");
    assert_eq!(stored.custody, BucketCustody::PlatformSealed);
    assert_eq!(stored.attached_by, OWNER);
}

/// **U1, B-X4.2.** `BucketOps::attach` used to echo `want.credential` in
/// its response — the label the operator installed, not a secret, but
/// deployment-side naming material a space owner has no use for. Checked
/// against the whole body rather than a single field, so a rename that
/// moved the label to a different key would still be caught.
#[tokio::test(flavor = "multi_thread")]
async fn the_attach_response_never_names_the_credential_label() {
    let h = harness("ordo_attach_no_label").await;
    let owner = token(OWNER, Some(NOW));

    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert!(
        !b.to_string().contains(CREDENTIAL),
        "the credential label reached the response: {b}"
    );
}

/// **U1, §2 B-C2.** `pathStyle` and `keyPrefix` are optional and round-trip
/// through the real door, into the row and back out in the response —
/// symmetric with every other target fact `endpoint`/`region`/`bucket`
/// already are.
#[tokio::test(flavor = "multi_thread")]
async fn an_attachment_through_the_door_can_state_path_style_and_a_key_prefix() {
    let h = harness("ordo_attach_pathstyle").await;
    let owner = token(OWNER, Some(NOW));

    let mut payload = body("acme-files", true);
    payload["pathStyle"] = json!(true);
    payload["keyPrefix"] = json!("incoming");
    let (status, b) = attach(&h.state, Some(&owner), payload).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["pathStyle"], json!(true));
    assert_eq!(b["rows"][0]["keyPrefix"], json!("incoming"));

    let stored = bucket::lookup(&h.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(stored.path_style, Some(true));
    assert_eq!(stored.key_prefix, Some("incoming".into()));
}

/// **U1, §2 B-C2.** A `keyPrefix` outside `ordo_blob::space_prefix`'s
/// charset is refused, not sanitised — the same rule and the same reason
/// a hostile space id is refused: a sanitiser turns two different hostile
/// inputs into one accepted prefix, and a prefix collision inside a
/// tenant's own bucket is the exact addressing mistake the prefix exists
/// to prevent.
#[tokio::test(flavor = "multi_thread")]
async fn a_hostile_key_prefix_is_refused_rather_than_sanitised() {
    let h = harness("ordo_attach_hostile_prefix").await;
    let owner = token(OWNER, Some(NOW));

    for hostile in ["../escape", "a/b", "a b", "a%2fb", "a\\b", ".."] {
        let mut payload = body("acme-files", true);
        payload["keyPrefix"] = json!(hostile);
        let (status, b) = attach(&h.state, Some(&owner), payload).await;
        assert!(status.is_client_error(), "{hostile:?}: {b}");
        assert_eq!(class(&b), "param_type_mismatch", "{hostile:?}: {b}");
    }

    // Nothing hostile was stored.
    assert!(
        bucket::lookup(&h.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
            .await
            .unwrap()
            .is_none()
    );

    // And a well-formed prefix is unaffected by the refusal existing.
    let mut payload = body("acme-files", true);
    payload["keyPrefix"] = json!("tenant_incoming-1");
    let (status, b) = attach(&h.state, Some(&owner), payload).await;
    assert_eq!(status, StatusCode::OK, "{b}");
}

/// **The idempotency ruling, through the door.** `PlatformExecutor` drops
/// the durable `ReplayLease`; this op is safe without it because it is
/// keyed on the target's identity — so a retry reports success, changes
/// nothing, and writes **no custody event**. Recording one would put a
/// false "storage custody changed" in the owner's stream every time a
/// client retried, which is the record that is supposed to mean something.
#[tokio::test(flavor = "multi_thread")]
async fn a_retry_is_unchanged_and_records_nothing() {
    let h = harness("ordo_attach_retry").await;
    let owner = token(OWNER, Some(NOW));

    let (status, _) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK);
    let after_first = custody_actions(&h);
    assert_eq!(after_first, vec!["bucket_attached".to_string()]);

    // The identical request again.
    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "unchanged");
    // `custody_actions` DRAINS, so this asks what was recorded *since the
    // first call* — which is the question, and is why the assertion is
    // against an empty list rather than against `after_first`.
    assert_eq!(
        custody_actions(&h),
        Vec::<String>::new(),
        "a retry is not a custody change and must not be recorded as one"
    );

    // A different target IS a change, and is recorded.
    let (status, b) = attach(&h.state, Some(&owner), body("acme-other", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "replaced");
    assert_eq!(
        custody_actions(&h),
        vec!["bucket_replaced".to_string()],
        "changing storage custody leaves a record; §2 pin (1) requires it"
    );
}

/// Each bad argument is refused **by the layer entitled to refuse it**,
/// and this test exists because the first version of this op got that
/// wrong: it raised `invalid_param` from the executor, which
/// `ApiError::from_exec` flattens to `internal`, so three separate caller
/// mistakes all arrived as 500s.
#[tokio::test(flavor = "multi_thread")]
async fn bad_arguments_are_refused_by_the_layer_that_may_refuse_them() {
    let h = harness("ordo_attach_refuse").await;
    let owner = token(OWNER, Some(NOW));

    // **The custody position is a bool, so a typo is the DOOR's refusal.**
    // That is the whole reason it is not a two-spelling text: text would
    // put the enum check in the handler, and the handler may not say
    // `invalid_param`.
    let mut payload = body("acme-files", true);
    payload["platformSealed"] = json!("platform-sealed");
    let (status, b) = attach(&h.state, Some(&owner), payload).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(class(&b), "invalid_param");

    // And omitting it is `missing_param`, not a default. The two positions
    // answer differently to what deleting the space does, so guessing
    // would be the platform deciding a disclosure on the owner's behalf.
    let mut payload = body("acme-files", true);
    payload.as_object_mut().unwrap().remove("platformSealed");
    let (status, b) = attach(&h.state, Some(&owner), payload).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(class(&b), "missing_param");

    // A credential nobody installed. `not_found`, in `callers_of`'s
    // non-enumerating shape — and the label set is not echoed back: a
    // caller guessing learns only that this guess was wrong.
    let mut payload = body("acme-files", true);
    payload["credential"] = json!("not-installed");
    let (status, b) = attach(&h.state, Some(&owner), payload).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{b}");
    assert_eq!(class(&b), "not_found");
    assert!(
        !b["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains(CREDENTIAL),
        "the refusal enumerated the installed credentials: {b}"
    );

    // A target that would not boot — checked here rather than at the first
    // upload, because discovering it then means discovering it in
    // production. Plaintext http off loopback carries the SigV4 credential
    // in the clear, and a captured signature is a valid request.
    //
    // `param_type_mismatch`, which the executor MAY say: the door typed
    // `$endpoint` as a `uri` and checked its shape, but "https, or http
    // only on loopback" is a fact about the target that no `ParamTy`
    // expresses.
    let mut payload = body("acme-files", true);
    payload["endpoint"] = json!("http://s3.example.com");
    let (status, b) = attach(&h.state, Some(&owner), payload).await;
    assert_eq!(class(&b), "param_type_mismatch", "{b}");
    assert!(status.is_client_error(), "{status}");

    // None of them stored anything.
    assert!(
        bucket::lookup(&h.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
            .await
            .unwrap()
            .is_none()
    );
}

/// A `uri`-typed parameter, bound at the door. Before `ParamTy::Uri`
/// existed this would have bound as unbounded text and reached the handler
/// as whatever the caller sent — the same defect `datetime` had, one type
/// over. This is the first op in the tree to declare one.
#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_is_bound_as_a_uri_at_the_door() {
    let h = harness("ordo_attach_uri").await;
    let owner = token(OWNER, Some(NOW));

    for hostile in ["not-a-uri", "", "https://exa mple.com"] {
        let mut payload = body("acme-files", true);
        payload["endpoint"] = json!(hostile);
        let (status, b) = attach(&h.state, Some(&owner), payload).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{hostile:?}: {b}");
        assert_eq!(
            class(&b),
            "invalid_param",
            "a `uri` parameter is validated at the DOOR, before the handler: {b}"
        );
    }
}

/// Every `CustodyAction` the stream recorded since the last call, as
/// `(scope, action)`. `drain_all` empties the recorder, which is what makes
/// "and nothing more was recorded" checkable by comparing successive drains
/// rather than by re-reading a growing list.
///
/// The scope comes back with the action because the property under test is
/// exactly which stream each landed on: the deployment's, never the tenant's.
fn custody_entries(h: &Harness) -> Vec<(ordo_secevent::Scope, String)> {
    h.stream
        .drain_all(ordo_commandlog::Timestamp(NOW))
        .iter()
        .filter_map(|e| match &e.kind {
            ordo_secevent::EventKind::CustodyAction { action } => {
                Some((e.scope.clone(), action.clone()))
            }
            _ => None,
        })
        .collect()
}

/// Just the verbs, for the tests that care how many custody changes
/// happened rather than where each was filed. One verb per **change**, not
/// per entry.
fn custody_actions(h: &Harness) -> Vec<String> {
    let mut out: Vec<String> = custody_entries(h)
        .into_iter()
        .map(|(_, action)| {
            action
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    out.dedup();
    out
}

// ── U6: the refusals, the probe and the record ───────────────────────────

/// **§10 B-X3.1.** A tenant pointed at the platform's own bucket gets an
/// attachment row and a consent page saying the bytes are in a bucket they
/// hold, and the bytes would be exactly where they already are. This is
/// `prepare_target`'s `ControlDatabase` refusal one tier over.
#[tokio::test(flavor = "multi_thread")]
async fn attaching_the_deployments_own_bucket_is_refused() {
    let h = harness("ordo_attach_own").await;
    let owner = token(OWNER, Some(NOW));

    let (status, b) = attach(&h.state, Some(&owner), body(OWN_BUCKET, true)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(class(&b), "bucket_refused", "{b}");
    assert!(
        bucket::lookup(&h.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
            .await
            .unwrap()
            .is_none(),
        "the platform's own bucket was recorded as this tenant's"
    );
    assert_eq!(custody_actions(&h), Vec::<String>::new());

    // Another bucket behind the same endpoint is unaffected: the bucket is
    // the unit, not the endpoint or the account.
    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
}

/// **A bucket may be shared, and a tenant's own perimeter may not be.**
///
/// This test asserted the opposite until 2026-09-25, and the reversal is
/// the point of it. D-D reads *"1 tenant 1 database. same for buckets.
/// database/schema not cluster."* The first build took "same for buckets"
/// to mean a bucket holds one tenant whatever prefix is stated. The owner's
/// clarification put the tiers one level lower: *"1 bucket can have folders
/// so 1 bucket defined per - but it could be the same and
/// bucketname/tenant would be the option."*
///
/// So a **bucket** answers to a **cluster** and a **folder** answers to a
/// **database**. `ordo_blob::object_key` already keys every object
/// `<space>/<handle>`, so two tenants in one bucket cannot collide however
/// their prefixes are set, and the sharing the clarification describes is
/// what the layout has always done.
///
/// What survives is D-S8, ruled *"agreed"*, which asks a different
/// question: a bucket declared as one tenant's **own perimeter** is theirs,
/// and the platform may not put somebody else inside it. A folder does not
/// answer that, because it is a disclosure rather than a collision.
#[tokio::test(flavor = "multi_thread")]
async fn a_bucket_is_shareable_but_a_tenants_own_perimeter_is_not() {
    let h = harness("ordo_attach_occupied").await;
    let owner = token(OWNER, Some(NOW));
    let first = ordo_crypto::SpaceId(DEFAULT_SPACE.into());

    // (custody already in the bucket, custody arriving, may it attach?)
    let cases = [
        (true, true, true),    // two platform-sealed tenants share: fine
        (false, false, false), // into someone's own perimeter: refused
        (false, true, false),  // sealed arriving into a perimeter: refused
        (true, false, false),  // claiming a perimeter someone is in: refused
    ];

    for (first_sealed, second_sealed, allowed) in cases {
        let other = ordo_crypto::SpaceId("spc_someone_else".into());
        bucket::ensure_table(&h.pool).await.unwrap();
        h.pool
            .rows("DELETE FROM \"$spacebucket\"")
            .await
            .expect("cleared");
        bucket::attach(
            &h.pool,
            &other,
            &bucket::Attachment {
                endpoint: ALLOWED_ENDPOINT.into(),
                region: "eu-west-1".into(),
                bucket: "shared-files".into(),
                credential: CREDENTIAL.into(),
                custody: if first_sealed {
                    BucketCustody::PlatformSealed
                } else {
                    BucketCustody::OwnPerimeter
                },
                path_style: None,
                key_prefix: None,
                attached_at: NOW as i64,
                attached_by: "mem_x".into(),
            },
        )
        .await
        .expect("the first tenant is in");

        let (status, b) = attach(&h.state, Some(&owner), body("shared-files", second_sealed)).await;
        if allowed {
            assert_eq!(
                status,
                StatusCode::OK,
                "{first_sealed}/{second_sealed}: sharing a bucket was refused: {b}"
            );
            assert!(
                bucket::lookup(&h.pool, &first).await.unwrap().is_some(),
                "the attachment did not land"
            );
        } else {
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{first_sealed}/{second_sealed}: {b}"
            );
            assert_eq!(class(&b), "bucket_refused", "{b}");
            // The holder is named: it is not the allowlist, the space id is
            // minted and opaque, and a deployment administrator resolving
            // this needs to know whose bucket it is.
            assert!(
                b["error"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("spc_someone_else"),
                "{b}"
            );
            assert!(
                bucket::lookup(&h.pool, &first).await.unwrap().is_none(),
                "{first_sealed}/{second_sealed}: a tenant landed inside a perimeter"
            );
        }
    }

    // And a bucket nobody is in attaches regardless of custody, so the
    // refusals above are about the perimeter rather than about everything.
    h.pool
        .rows("DELETE FROM \"$spacebucket\"")
        .await
        .expect("cleared");
    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", false)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
}

/// **D-S6, and §10 B-X3.4.** An endpoint the operator did not name is
/// refused **with one reason and no detail**: the refusal must not say
/// whether the host was absent, malformed, or the wrong one of several,
/// because a distinguishable refusal is an oracle that maps the allowlist
/// one attempt at a time. The allowlist itself is the *database* tier's,
/// tied in as the owner asked.
#[tokio::test(flavor = "multi_thread")]
async fn an_endpoint_off_the_allowlist_is_refused_with_one_reason() {
    let h = harness("ordo_attach_allowlist").await;
    let owner = token(OWNER, Some(NOW));

    let mut seen = std::collections::BTreeSet::new();
    for off in [
        "https://s3.us-east-1.amazonaws.com",
        "https://169.254.169.254",
        "https://gateway.acme.com:9000",
    ] {
        let mut payload = body("acme-files", true);
        payload["endpoint"] = json!(off);
        let (status, b) = attach(&h.state, Some(&owner), payload).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{off}: {b}");
        assert_eq!(class(&b), "bucket_refused", "{off}: {b}");
        let message = b["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // No detail: not the host, not the allowlist, not the variable.
        assert!(
            !message.contains(off),
            "the refusal echoed the host: {message}"
        );
        assert!(
            !message.contains(ALLOWED_ENDPOINT_HOST),
            "the refusal named what IS allowed: {message}"
        );
        assert!(
            !message.contains("ORDO_TENANT_ATTACH_HOSTS")
                && !message.contains("ORDO_PG_ATTACH_HOSTS"),
            "the refusal named the control that refused it: {message}"
        );
        seen.insert(message);
    }
    // **One reason**, which is what makes it useless as an oracle: three
    // different rejected endpoints produced one message.
    assert_eq!(
        seen.len(),
        1,
        "the refusal distinguishes endpoints: {seen:?}"
    );

    // Nothing was stored, and an endpoint that IS on the list still works,
    // so the assertions above are not passing because everything is
    // refused.
    assert!(
        bucket::lookup(&h.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
            .await
            .unwrap()
            .is_none()
    );
    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
}

/// **D-S6's second half.** `S3Config::new` permits plaintext http to
/// `localhost`, `127.0.0.1`, `::1`, `*.localhost` and the bare name
/// `minio`, which the self-host story needs on a laptop and which in a pod
/// is a request into the deployment's own network with the SigV4 credential
/// in the clear. Behind a dev flag, the shape `fs:` already uses — and the
/// allowlist still has to name the host, so the flag widens nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn a_loopback_endpoint_needs_the_dev_flag() {
    let owner = token(OWNER, Some(NOW));
    let mut payload = body("acme-files", true);
    payload["endpoint"] = json!("http://127.0.0.1:9000");

    // On the allowlist, and still refused without the flag.
    let strict = harness_with(
        "ordo_attach_local_off",
        HostAllowlist::of("127.0.0.1"),
        false,
    )
    .await;
    let (status, b) = attach(&strict.state, Some(&owner), payload.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(class(&b), "bucket_refused", "{b}");
    assert!(
        bucket::lookup(&strict.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
            .await
            .unwrap()
            .is_none()
    );

    // With the flag, the same call lands.
    let dev = harness_with("ordo_attach_local_on", HostAllowlist::of("127.0.0.1"), true).await;
    let (status, b) = attach(&dev.state, Some(&owner), payload.clone()).await;
    assert_eq!(status, StatusCode::OK, "{b}");

    // And the flag is not a way past the allowlist: a loopback endpoint the
    // operator did not name is still refused with the flag set.
    let dev_no_list = harness_with("ordo_attach_local_nolist", HostAllowlist::of(""), true).await;
    let (status, b) = attach(&dev_no_list.state, Some(&owner), payload).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(class(&b), "bucket_refused", "{b}");
}

/// **§5 B-F5 and D-S7.** The attach proves the credential signs at the
/// target *before* the row is written, so a tenant does not discover a
/// credential that cannot sign at their first upload — which has already
/// consumed their bytes by the time it fails. And the two failures answer
/// differently, because an operator acts on them differently: a bucket that
/// refused the credential is 400 and never lands until somebody changes the
/// policy; a bucket that could not be reached is 503 and the identical call
/// lands.
#[tokio::test(flavor = "multi_thread")]
async fn an_attach_probes_the_credential_before_the_row_is_written() {
    let h = harness("ordo_attach_probe").await;
    let owner = token(OWNER, Some(NOW));
    let space = ordo_crypto::SpaceId(DEFAULT_SPACE.into());

    *h.probe.answer.lock().unwrap() = Err(ordo_blob::BlobError::Refused(
        "HTTP 403: AccessDenied acme-prod".into(),
    ));
    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(class(&b), "bucket_refused", "{b}");
    // **§10 B-X4.4: the store's own text never reaches the caller.** It can
    // name the credential's principal, the bucket and the account.
    let message = b["error"]["message"].as_str().unwrap_or_default();
    assert!(!message.contains("AccessDenied"), "{message}");
    assert!(!message.contains(CREDENTIAL), "{message}");
    // Nothing was written, which is the ordering this test is named for.
    assert!(
        bucket::lookup(&h.pool, &space).await.unwrap().is_none(),
        "the row was written before the probe answered"
    );
    assert_eq!(custody_actions(&h), Vec::<String>::new());

    // An unreachable bucket is the other class: 503, retryable.
    *h.probe.answer.lock().unwrap() =
        Err(ordo_blob::BlobError::Transport("HTTP 503: gateway".into()));
    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{b}");
    assert_eq!(class(&b), "space_unavailable", "{b}");
    assert!(bucket::lookup(&h.pool, &space).await.unwrap().is_none());

    // And the probe was asked about the target the resolver will address —
    // the bucket, and the prefix the row states — rather than one this op
    // assembled for itself.
    *h.probe.answer.lock().unwrap() = Ok(());
    let mut payload = body("acme-files", true);
    payload["keyPrefix"] = json!("incoming");
    let (status, b) = attach(&h.state, Some(&owner), payload).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(
        *h.probe.seen.lock().unwrap(),
        Some((
            ALLOWED_ENDPOINT_HOST.to_string(),
            "acme-files".to_string(),
            Some("incoming".to_string())
        ))
    );
}

/// **A space that already holds files may change its bucket** (ruled
/// 2026-10-06, `st-attach-existing` ask 2, lifting the D-S2 refusal). Each
/// `$blobref` row records the store its bytes went to (25ac), so the old
/// files stay where they are and are read and cleaned up there. The answer
/// says how many stay put. A regression answers `bucket_refused`.
#[tokio::test(flavor = "multi_thread")]
async fn attaching_a_space_that_already_holds_objects_is_allowed() {
    let h = harness("ordo_attach_objects").await;
    let owner = token(OWNER, Some(NOW));
    let space = ordo_crypto::SpaceId(DEFAULT_SPACE.into());

    // The reference table, **through the platform's own DDL** rather than a
    // hand-written one, so this test cannot pass against a table of its own
    // invention.
    h.pool
        .rows(ordo_crypto::blobref::BLOB_REF_DDL)
        .await
        .expect("reference table");
    h.pool
        .rows("DELETE FROM \"$blobref\"")
        .await
        .expect("cleared");
    h.pool.rows(&blobref_row("aa")).await.expect("one object");

    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "attached");
    assert_eq!(b["rows"][0]["filesStayingPut"], 1, "{b}");
    assert!(bucket::lookup(&h.pool, &space).await.unwrap().is_some());
    let _ = custody_actions(&h);

    // A retry of the standing attachment is `unchanged` and records
    // nothing, however many files the tenant has since uploaded.
    h.pool.rows(&blobref_row("bb")).await.unwrap();
    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "unchanged");
    assert_eq!(custody_actions(&h), Vec::<String>::new());

    // And a genuine change lands, recorded, with the count of files that
    // stay in the bucket they were written to.
    let (status, b) = attach(&h.state, Some(&owner), body("acme-other", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "replaced");
    assert_eq!(b["rows"][0]["filesStayingPut"], 2, "{b}");
    assert_eq!(
        bucket::lookup(&h.pool, &space)
            .await
            .unwrap()
            .unwrap()
            .bucket,
        "acme-other"
    );

    h.pool.rows("DELETE FROM \"$blobref\"").await.unwrap();
}

/// **§10 B-X6, as ruled 2026-09-29 (22k): a bucket attach is recorded on the
/// deployment's stream and not on the tenant's own**, and the record carries
/// no secret.
///
/// The deployment's stream is where an owner lists custody changes across
/// every tenant, and it carries the space as the event's object. What a
/// tenant shows its own users about where its files went is the
/// application's to say. The tenant's own **chain** records nothing either
/// (D-F).
#[tokio::test(flavor = "multi_thread")]
async fn a_bucket_attach_is_recorded_on_the_deployments_stream_and_not_the_tenants() {
    let h = harness("ordo_attach_record").await;
    let owner = token(OWNER, Some(NOW));

    let mut payload = body("acme-files", false);
    payload["keyPrefix"] = json!("incoming");
    let (status, b) = attach(&h.state, Some(&owner), payload).await;
    assert_eq!(status, StatusCode::OK, "{b}");

    let entries = custody_entries(&h);
    assert_eq!(
        entries.len(),
        1,
        "one record, the deployment's: {entries:?}"
    );
    assert_eq!(
        entries[0].0,
        ordo_secevent::Scope::Deployment,
        "{entries:?}"
    );
    assert!(
        !entries
            .iter()
            .any(|(s, _)| matches!(s, ordo_secevent::Scope::Space(_))),
        "the tenant's own security stream must carry no bucket attach: {entries:?}"
    );

    for (scope, action) in &entries {
        // **Everything §10 B-X6 asks for rides the action string**, because
        // `CustodyAction` carries one string and is hash-chained under a
        // tag: widening the variant would change the encoding of every
        // event already written.
        assert!(action.starts_with("bucket_attached "), "{action}");
        assert!(action.contains("acme-files"), "{action}");
        assert!(action.contains(ALLOWED_ENDPOINT), "{action}");
        assert!(action.contains("eu-west-1"), "{action}");
        assert!(action.contains("own-perimeter"), "{scope:?}: {action}");
        assert!(action.contains("incoming"), "{action}");
        assert!(action.contains("occupancy=1"), "{action}");
        // The credential **label** is in it and the material is not: the
        // record has to say which credential the platform will sign with,
        // and a label is what that is.
        assert!(action.contains(CREDENTIAL), "{action}");
        assert!(
            !action.contains("s3cr3t"),
            "a secret reached the stream: {action}"
        );
        assert!(
            !action.contains("AKIAEXAMPLE"),
            "a secret reached the stream: {action}"
        );
    }

    // And the drain above really emptied the recorder, so "one entry" is
    // one rather than the first of more.
    assert!(h
        .stream
        .drain_all(ordo_commandlog::Timestamp(NOW))
        .is_empty());
}

/// The deployment's record of a bucket change names the member who made
/// the call. It is the only record of the change since 73fbc0dc, and a
/// record that named the platform as its subject said nothing about who
/// asked (audit 2026-10-06). The attach and the detach both.
#[tokio::test(flavor = "multi_thread")]
async fn the_deployments_record_of_a_bucket_change_names_the_member_who_asked() {
    let h = harness("ordo_attach_record_subject").await;
    let owner = token(OWNER, Some(NOW));

    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", false)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    let (status, b) = detach(&h.state, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK, "{b}");

    let entries: Vec<_> = h
        .stream
        .drain_all(ordo_commandlog::Timestamp(NOW))
        .into_iter()
        .filter(|e| matches!(e.kind, ordo_secevent::EventKind::CustodyAction { .. }))
        .collect();
    assert_eq!(entries.len(), 2, "one attach, one detach: {entries:?}");
    for e in &entries {
        assert_eq!(e.scope, ordo_secevent::Scope::Deployment, "{e:?}");
        assert_eq!(
            e.subject,
            ordo_secevent::Subject::member(OWNER),
            "the record names the member who asked: {e:?}"
        );
    }
}

/// One `$blobref` row for [`DEFAULT_SPACE`], with every `NOT NULL` column
/// filled. The values are placeholders because `objects_held` counts rows
/// and reads none of them; what matters is that the row is the shape the
/// real table has, so this test cannot pass against a table of its own
/// invention.
fn blobref_row(handle: &str) -> String {
    format!(
        "INSERT INTO \"$blobref\" \
         (\"$space\", \"ref\", \"epoch\", \"entity\", \"field\", \"mime\", \"size\", \
          \"integrity\", \"bytes\", \"created_at\") \
         VALUES ('{DEFAULT_SPACE}', '\\x{handle}'::bytea, 1, 'Doc', 'file', '\\x00'::bytea, \
                 '\\x00'::bytea, '\\x00'::bytea, 1, {NOW})"
    )
}

// ── detach (owner ruling 2026-10-05, `bucket-detach`) ────────────────────

async fn detach(state: &ApiState, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(format!("/op/{DETACH_BUCKET}"))
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = router(state.clone())
        .oneshot(req.body(Body::from("{}")).unwrap())
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

/// **Detach is the owner's, step-up gated, as the attach is.** A manager, an
/// editor and a viewer are refused with the door's one answer, an owner
/// without presence is refused, and the attachment stands through all of
/// them.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_with_presence_may_detach_a_bucket() {
    let h = harness("ordo_detach_who").await;
    let owner = token(OWNER, Some(NOW));
    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");

    for who in [MANAGER, EDITOR, VIEWER] {
        let (status, b) = detach(&h.state, Some(&token(who, Some(NOW)))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who} reached it: {b}");
        assert_eq!(class(&b), "access_denied", "{who}");
    }
    let (status, b) = detach(&h.state, Some(&token(OWNER, None))).await;
    assert_ne!(status, StatusCode::OK, "a stale token detached: {b}");
    let (status, _) = detach(&h.state, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    assert!(
        bucket::lookup(&h.pool, &ordo_crypto::SpaceId(DEFAULT_SPACE.into()))
            .await
            .unwrap()
            .is_some(),
        "the attachment stands"
    );
}

/// **A detach returns the tenant to platform custody and is recorded like
/// the attach**: the row is gone, the answer says `platform`, and the
/// custody event is on the deployment's stream only, naming the
/// bucket that was detached. A retry is `unchanged` and records nothing,
/// and a later attach is an `attached`, not a `replaced`.
#[tokio::test(flavor = "multi_thread")]
async fn a_detach_returns_the_tenant_to_platform_custody_and_is_recorded() {
    let h = harness("ordo_detach_ok").await;
    let owner = token(OWNER, Some(NOW));
    let space = ordo_crypto::SpaceId(DEFAULT_SPACE.into());

    // Nothing attached: an answer, not a refusal, and no record.
    let (status, b) = detach(&h.state, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "unchanged");
    assert_eq!(b["rows"][0]["custody"], "platform");

    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", false)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    let _ = custody_entries(&h);

    let (status, b) = detach(&h.state, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    let row = &b["rows"][0];
    assert_eq!(row["outcome"], "detached");
    assert_eq!(row["custody"], "platform");
    assert_eq!(row["bucket"], "acme-files");
    assert!(
        !b.to_string().contains(CREDENTIAL),
        "the answer never names the credential label: {b}"
    );
    assert!(bucket::lookup(&h.pool, &space).await.unwrap().is_none());

    let entries = custody_entries(&h);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(
        entries[0].0,
        ordo_secevent::Scope::Deployment,
        "{entries:?}"
    );
    for (_, action) in &entries {
        assert!(action.starts_with("bucket_detached "), "{action}");
        assert!(action.contains("acme-files"), "{action}");
        assert!(action.contains("own-perimeter"), "{action}");
    }

    // A retry is unchanged and records nothing.
    let (status, b) = detach(&h.state, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "unchanged");
    assert_eq!(custody_actions(&h), Vec::<String>::new());

    // Attaching again is a fresh attach.
    let (status, b) = attach(&h.state, Some(&owner), body("acme-other", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "attached");
}

/// **A detach while the tenant holds files is allowed** (ruled 2026-10-06,
/// `st-attach-existing` ask 2). The files stay in the detached bucket and
/// are still read and cleaned up there; the answer says how many. A
/// regression answers `bucket_refused` and leaves the attachment standing.
#[tokio::test(flavor = "multi_thread")]
async fn detaching_a_space_that_holds_objects_is_allowed() {
    let h = harness("ordo_detach_objects").await;
    let owner = token(OWNER, Some(NOW));
    let space = ordo_crypto::SpaceId(DEFAULT_SPACE.into());
    h.pool
        .rows(ordo_crypto::blobref::BLOB_REF_DDL)
        .await
        .expect("reference table");
    h.pool
        .rows("DELETE FROM \"$blobref\"")
        .await
        .expect("cleared");

    let (status, b) = attach(&h.state, Some(&owner), body("acme-files", true)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    let _ = custody_entries(&h);
    h.pool.rows(&blobref_row("cc")).await.expect("one object");

    let (status, b) = detach(&h.state, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "detached");
    assert_eq!(b["rows"][0]["filesStayingPut"], 1, "{b}");
    assert!(bucket::lookup(&h.pool, &space).await.unwrap().is_none());
    assert!(!custody_actions(&h).is_empty(), "the detach is recorded");

    h.pool.rows("DELETE FROM \"$blobref\"").await.unwrap();
}
