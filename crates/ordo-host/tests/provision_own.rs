//! Built-in provisioning (`plan-provisioning-on-signup.md`), increments P0
//! and P1.
//!
//! **P0**, the deployment owner named at boot: `ORDO_DEPLOYMENT_OWNERS`, read
//! into `RuntimeConfig::deployment_owners`, becomes owner rows in the
//! deployment's own space after the system space is ensured, on the real
//! assembled runtime.
//!
//! **P1**, `provision_own` and the latch: the `own_of` column and its partial
//! unique index, `tenants::reserve` and `tenants::own_of`, and
//! `TenantOps::provision_own` over real Postgres databases.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::KeyProvider as _;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::runtime::{self, CustodyChoice, RuntimeConfig};

const SPACE: &str = "acme";
const PASSPHRASE: &str = "the provision-own suite's passphrase";

fn pg_cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 6,
        ..PoolConfig::from_env()
    }
}

/// A `RuntimeConfig` built by hand, as `tests/supervisor.rs` does: custody is
/// a sealed master over files this test writes, because `set_var` is
/// `unsafe` and this workspace forbids `unsafe`. So the environment variable
/// is exercised through the field `RuntimeConfig::from_env` fills from it.
fn config(db: &str, owners: &[&str]) -> RuntimeConfig {
    RuntimeConfig {
        blob_routing: ordo_host::runtime::BlobRouting::Single,
        pg: pg_cfg(db),
        addr: "127.0.0.1:0".parse().expect("addr"),
        shutdown_grace: Duration::from_secs(5),
        space: ordo_crypto::SpaceId(SPACE.into()),
        custody: CustodyChoice::SealedMaster {
            path: key_file(),
            passphrase: pass_file(),
        },
        token_secret: [7u8; 32],
        page_key: ordo_api::PageKey::from_secret(b"ordo-provision-own-test"),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "provision-own-test".into(),
        usage_flush: Duration::from_secs(60),
        usage_rollup: Duration::from_secs(3600),
        sync: Default::default(),
        auth: None,
        credentials: None,
        presence: Default::default(),
        pausing: Default::default(),
        delegable: Default::default(),
        access_template: Default::default(),
        deployment_owners: owners.iter().map(|o| o.to_string()).collect(),
        provisioning: Default::default(),
        recovery: Default::default(),
        second_factor: Default::default(),
        mail: None,
    }
}

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ordo-provown-{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("tmp");
    d
}

fn key_file() -> std::path::PathBuf {
    dir().join("master.key")
}

/// Written once per test binary. Tests run in parallel, and a rewrite
/// truncates the file before it fills it, so a second test could read it
/// empty while the first was writing.
fn pass_file() -> std::path::PathBuf {
    static ONCE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let p = dir().join("pass");
        std::fs::write(&p, PASSPHRASE).expect("write");
        p
    })
    .clone()
}

fn master() -> [u8; 32] {
    use std::sync::OnceLock;
    static ONCE: OnceLock<[u8; 32]> = OnceLock::new();
    *ONCE.get_or_init(|| {
        let path = key_file();
        let _ = std::fs::remove_file(&path);
        ordo_crypto::master::seal_master(&path, "provision-own-suite", PASSPHRASE.as_bytes())
            .expect("sealed");
        ordo_crypto::master::open_sealed_master(&path, PASSPHRASE.as_bytes()).expect("opens")
    })
}

/// A deployment an operator has applied and `keys init`ed, under a master
/// key this test holds, serving the market fixture in `acme`.
async fn deployment_ready(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    let source = include_str!("../../ordo-conformance/fixtures/market.ordo");

    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("clean");
    let plain = Fixture::from_source(source, "").expect("compiles");
    pg.exec(&plain.ddl).expect("ddl");
    pg.exec(ordo_crypto::SEALED_PAYLOAD_DDL).expect("payloads");

    let pool = Pool::connect(pg_cfg(db)).await.expect("pool");
    ordo_exec::keystore::ensure_tables(&pool)
        .await
        .expect("custody table");
    let provider = ordo_crypto::LocalKeyProvider::from_master(
        master(),
        Box::new(ordo_exec::keystore::PgKeyStore::open(pg_cfg(db)).expect("store")),
    );
    provider
        .create_space(&ordo_crypto::SpaceId(SPACE.into()))
        .expect("custody");
    let keys: Arc<dyn ordo_crypto::KeyProvider> = Arc::new(provider);

    let mut f = Fixture::load().expect("fixture");
    f.keys = keys.clone();
    let f = f.in_space(SPACE);
    f.seed_into(&pg).expect("seed");

    ordo_exec::registry::ensure_tables(&pool)
        .await
        .expect("registry");
    let doc = ordo_plan::artifact::program_artifact(&f.schema, &f.registry).expect("artifact");
    let ddl = f.ddl.clone();
    let art = ordo_exec::registry::Artifact {
        version: ordo_plan::artifact::program_version(&doc, &ddl),
        artifact: ordo_plan::artifact::to_bytes(&doc),
        source: source.to_string(),
        ddl,
        applied_at: ordo_commandlog::Timestamp(1),
        actor: "operator:provision-own".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:provision-own",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
    pool
}

/// The home space's `members` rows for one principal: `(id, role, status)`.
async fn rows_of(pool: &Pool, principal: &str) -> Vec<(String, String, String)> {
    pool.rows(&format!(
        "SELECT \"id\", \"role\", \"status\" FROM \"members\" \
         WHERE \"$space\" = '{SPACE}' AND \"principal\" = '{principal}'"
    ))
    .await
    .expect("members")
    .into_iter()
    .map(|r| {
        let c = |i: usize| r[i].clone().unwrap_or_default();
        (c(0), c(1), c(2))
    })
    .collect()
}

// ── P0: the deployment owner named at boot ───────────────────────────────

/// Every test here boots a whole runtime with its own pools, so run in
/// parallel they exhaust the database's connections ("too many clients").
/// Each test holds this for its body, which serialises this file only.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The stand-in Discord, for the outside-provider sign-ups below.
mod oauth2_support;

/// A fresh deployment has no owner now that first-comer is off by default.
/// Naming a principal in `ORDO_DEPLOYMENT_OWNERS` makes it an active owner
/// of the deployment's own space on the boot, before the door reads
/// `members`. A principal nothing names holds nothing, so the row is not a
/// side effect of booting.
#[tokio::test(flavor = "multi_thread")]
async fn a_principal_named_in_the_environment_owns_the_home_space_after_boot() {
    let _serial = crate::SERIAL.lock().await;
    const DB: &str = "ordo_pown_named";
    let pool = deployment_ready(DB).await;
    assert!(rows_of(&pool, "ada_deploy").await.is_empty());

    let rt = runtime::assemble(config(DB, &["ada_deploy", "grace_deploy"]))
        .await
        .expect("assembles");

    for who in ["ada_deploy", "grace_deploy"] {
        let rows = rows_of(&pool, who).await;
        assert_eq!(rows.len(), 1, "{who} holds one row: {rows:?}");
        assert_eq!(rows[0].1, "owner", "{who}");
        assert_eq!(rows[0].2, "active", "{who}");
    }
    assert!(
        rows_of(&pool, "someone_else").await.is_empty(),
        "an unnamed principal holds nothing"
    );
    drop(rt);
}

/// A restart with the same variable writes nothing: the row keeps its id,
/// and there is still one. Then a second principal is added by the same
/// variable growing, and the first is untouched.
#[tokio::test(flavor = "multi_thread")]
async fn the_variable_adds_and_a_second_boot_writes_nothing() {
    let _serial = crate::SERIAL.lock().await;
    const DB: &str = "ordo_pown_idem";
    let pool = deployment_ready(DB).await;

    let rt = runtime::assemble(config(DB, &["ada_deploy"]))
        .await
        .expect("first boot");
    let first = rows_of(&pool, "ada_deploy").await;
    assert_eq!(first.len(), 1);
    drop(rt);

    let rt = runtime::assemble(config(DB, &["ada_deploy"]))
        .await
        .expect("second boot");
    assert_eq!(
        rows_of(&pool, "ada_deploy").await,
        first,
        "a restart wrote nothing: the same row, the same id"
    );
    drop(rt);

    let rt = runtime::assemble(config(DB, &["ada_deploy", "grace_deploy"]))
        .await
        .expect("third boot");
    assert_eq!(rows_of(&pool, "ada_deploy").await, first);
    assert_eq!(rows_of(&pool, "grace_deploy").await.len(), 1);
    drop(rt);
}

/// The variable adds and never removes. An owner removed with
/// `std.space.remove` (a `DELETE` of the row) is not brought back by a boot
/// whose variable no longer names them, and is brought back by one that
/// names them again: the variable is the deployment's own statement of who
/// owns it, so it is edited to match.
#[tokio::test(flavor = "multi_thread")]
async fn a_removed_owner_is_not_re_added_unless_named_again() {
    let _serial = crate::SERIAL.lock().await;
    const DB: &str = "ordo_pown_removed";
    let pool = deployment_ready(DB).await;

    let rt = runtime::assemble(config(DB, &["ada_deploy", "grace_deploy"]))
        .await
        .expect("first boot");
    drop(rt);
    pool.rows(&format!(
        "DELETE FROM \"members\" WHERE \"$space\" = '{SPACE}' AND \"principal\" = 'ada_deploy'"
    ))
    .await
    .expect("removed");

    // The variable no longer names her.
    let rt = runtime::assemble(config(DB, &["grace_deploy"]))
        .await
        .expect("second boot");
    assert!(
        rows_of(&pool, "ada_deploy").await.is_empty(),
        "a removed owner the variable no longer names stays removed"
    );
    assert_eq!(rows_of(&pool, "grace_deploy").await.len(), 1);
    drop(rt);

    // Named again.
    let rt = runtime::assemble(config(DB, &["ada_deploy", "grace_deploy"]))
        .await
        .expect("third boot");
    let rows = rows_of(&pool, "ada_deploy").await;
    assert_eq!(rows.len(), 1, "named again, she is an owner again");
    assert_eq!(rows[0].1, "owner");
    drop(rt);
}

/// The act is recorded once, when the row is written, and not on the boot
/// that finds it there. Read from the recorder the boot hands the drain.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_owner_is_recorded_when_written_and_not_when_found() {
    let _serial = crate::SERIAL.lock().await;
    const DB: &str = "ordo_pown_recorded";
    let pool = deployment_ready(DB).await;
    let mut f = Fixture::load().expect("fixture");
    f.keys = Arc::new(ordo_crypto::TestProvider::with_space(SPACE));
    let owners = vec!["ada_deploy".to_string()];
    let recorder = ordo_secevent::Recorder::default();
    let at = ordo_commandlog::Timestamp(1_700_000_000);

    let first = ordo_host::provision::ensure_deployment_owners(
        &pool,
        f.keys.as_ref(),
        SPACE,
        &owners,
        Some(&recorder),
        at.0,
    )
    .await
    .expect("first");
    assert_eq!(
        first,
        vec![(
            "ada_deploy".to_string(),
            ordo_host::provision::OwnerOutcome::Added
        )]
    );
    let seen = format!("{:?}", recorder.drain_all(at));
    assert!(seen.contains("deployment-owner"), "recorded: {seen}");
    assert!(seen.contains("principal=ada_deploy"), "names her: {seen}");

    let second = ordo_host::provision::ensure_deployment_owners(
        &pool,
        f.keys.as_ref(),
        SPACE,
        &owners,
        Some(&recorder),
        at.0,
    )
    .await
    .expect("second");
    assert_eq!(
        second,
        vec![(
            "ada_deploy".to_string(),
            ordo_host::provision::OwnerOutcome::AlreadyOwner
        )]
    );
    assert!(
        recorder.drain_all(at).is_empty(),
        "a boot that finds the row records nothing"
    );
}

/// A malformed id refuses; a blank entry and a repeat do not.
#[test]
fn the_variable_is_parsed_strictly_and_kindly() {
    use ordo_host::provision::parse_deployment_owners as parse;
    assert_eq!(
        parse(" ada , grace,,ada ").expect("parses"),
        vec!["ada".to_string(), "grace".to_string()]
    );
    assert!(parse("").expect("empty is nobody").is_empty());
    let refused = parse("ada, bad id").expect_err("a space is not addressable");
    assert!(refused.contains("bad id"), "{refused}");
    assert!(parse("a'b").is_err());
}

/// Step 8 (owner, 2026-10-30): a door without the identity service reads
/// `ORDO_DEPLOYMENT_OWNERS` exactly as before, a malformed id refusing.
/// A door with `ORDO_IDENTITY_URL` set never parses it, so even a
/// malformed value cannot refuse the boot, and the boot log names it.
#[test]
fn deployment_owners_are_read_until_the_identity_service_is_set_then_ignored_with_a_log_line() {
    use ordo_host::provision::{deployment_owners_from, ignored_deployment_owners_note};
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    };
    // Without the identity service: read, and strict.
    let old = env(&[("ORDO_DEPLOYMENT_OWNERS", "ada,grace")]);
    assert_eq!(
        deployment_owners_from(&old).expect("read"),
        vec!["ada".to_string(), "grace".to_string()]
    );
    assert_eq!(ignored_deployment_owners_note(&old), None);
    let bad = env(&[("ORDO_DEPLOYMENT_OWNERS", "bad id")]);
    assert!(deployment_owners_from(&bad).is_err());
    // A blank identity URL is no identity service.
    let blank = env(&[
        ("ORDO_DEPLOYMENT_OWNERS", "ada"),
        ("ORDO_IDENTITY_URL", " "),
    ]);
    assert_eq!(deployment_owners_from(&blank).expect("read"), vec!["ada"]);
    assert_eq!(ignored_deployment_owners_note(&blank), None);

    // With the identity service: ignored, never refused, and named.
    let new = env(&[
        ("ORDO_DEPLOYMENT_OWNERS", "bad id"),
        ("ORDO_IDENTITY_URL", "http://127.0.0.1:8082"),
    ]);
    assert!(deployment_owners_from(&new)
        .expect("never refused")
        .is_empty());
    let note = ignored_deployment_owners_note(&new).expect("a log line");
    assert!(
        note.starts_with("ORDO_DEPLOYMENT_OWNERS is set and ignored"),
        "{note}"
    );
    assert!(note.contains("ORDO_IDENTITY_URL"), "{note}");
    assert!(note.contains("ordo-identity-service invite"), "{note}");
    // Nothing to say when the setting is not there.
    let clean = env(&[("ORDO_IDENTITY_URL", "http://127.0.0.1:8082")]);
    assert_eq!(ignored_deployment_owners_note(&clean), None);
}

// ── P1: provision_own and the latch ──────────────────────────────────────

mod own {
    use super::*;
    use ordo_host::deploy::Deployment;
    use ordo_host::tenant_ops::{OwnTenant, Routing, TenantCluster, TenantOps};
    use ordo_host::tenants::{self, State};

    /// The deployment's own space, which `provision_own` never binds.
    pub(super) const HOME: &str = "ordo_home";

    pub(super) struct H {
        pub(super) pool: Pool,
        pub(super) deployment: Arc<Deployment>,
        pub(super) ops: Arc<TenantOps>,
        pub(super) recorder: Arc<ordo_secevent::Recorder>,
        pub(super) fixture: Arc<Fixture>,
    }

    fn routing(deployment: &Arc<Deployment>, recorder: &Arc<ordo_secevent::Recorder>) -> Routing {
        Routing {
            serving: deployment.clone(),
            credentials: ordo_exec::route::WithOwnCredential::wrapping(
                Arc::new(ordo_exec::route::Installed::new().as_configured("local")),
                PoolConfig::from_env().password.clone(),
            ),
            template: PoolConfig {
                size: 2,
                ..PoolConfig::from_env()
            },
            hosts: ordo_host::provision::HostAllowlist::of(&PoolConfig::from_env().host),
            default_cluster: TenantCluster::from_env(&PoolConfig::from_env()),
            stream: Some(recorder.clone()),
        }
    }

    pub(super) async fn harness(db: &str) -> H {
        let f = Arc::new(Fixture::load().expect("fixture"));
        let pg = Pg::scoped(db);
        f.reset(&pg).expect("seeded");
        // Room for the control connections a burst of provisions holds:
        // the apply lock takes one for as long as a provision runs.
        let pool = Pool::connect(PoolConfig {
            db: pg.db,
            size: 12,
            acquire_timeout: Duration::from_secs(30),
            ..PoolConfig::from_env()
        })
        .await
        .expect("control pool");
        tenants::ensure_table(&pool).await.expect("$spaces");
        ordo_exec::spacedb::ensure_table(&pool)
            .await
            .expect("$spacedb");
        let deployment = Deployment::pinned(
            "provision-own-v1",
            f.clone(),
            ordo_exec::Executor {
                pool: pool.clone(),
                schema: f.schema.clone(),
                registry: f.registry.clone(),
                keys: f.keys.clone(),
                space: ordo_conformance::fixture_space(),
            },
        );
        let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
        let ops = TenantOps::with_routing(
            pool.clone(),
            f.keys.clone(),
            HOME,
            routing(&deployment, &recorder),
        );
        H {
            pool,
            deployment,
            ops,
            recorder,
            fixture: f,
        }
    }

    /// A second server over the same registry: its own lease holder and its
    /// own in-process claims, as another replica has.
    pub(super) fn second_server(h: &H) -> Arc<TenantOps> {
        TenantOps::with_routing(
            h.pool.clone(),
            h.fixture.keys.clone(),
            HOME,
            routing(&h.deployment, &h.recorder),
        )
    }

    pub(super) fn drop_database(name: &str) {
        let _ = Pg {
            db: "postgres".into(),
            ..Pg::default()
        }
        .exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
    }

    async fn database_exists(pool: &Pool, name: &str) -> bool {
        !pool
            .rows(&format!(
                "SELECT 1 FROM pg_database WHERE datname = '{name}'"
            ))
            .await
            .expect("pg_database")
            .is_empty()
    }

    /// Own-tenant rows for a principal, in every state.
    pub(super) async fn own_rows(pool: &Pool, principal: &str) -> Vec<(String, String)> {
        pool.rows(&format!(
            "SELECT \"$space\", \"state\" FROM \"$spaces\" WHERE \"own_of\" = '{principal}' \
             ORDER BY \"created_at\", \"$space\""
        ))
        .await
        .expect("rows")
        .into_iter()
        .map(|r| {
            (
                r[0].clone().unwrap_or_default(),
                r[1].clone().unwrap_or_default(),
            )
        })
        .collect()
    }

    /// The latch is the index, not a habit of the caller. Eight `reserve`s
    /// race with eight different ids for one principal, straight at the
    /// registry: exactly one writes a row. A tenant nobody is the own tenant
    /// of is not held to it, and a `failed` own tenant releases it.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_registry_admits_one_live_own_tenant_per_principal() {
        let _serial = crate::SERIAL.lock().await;
        let h = harness("ordo_pown_latch").await;
        let mut tasks = Vec::new();
        for n in 0..8 {
            let pool = h.pool.clone();
            tasks.push(tokio::spawn(async move {
                tenants::reserve(
                    &pool,
                    &format!("spc_race{n}"),
                    "Racer",
                    "ada",
                    1_000,
                    Some("ada"),
                )
                .await
                .expect("reserve")
            }));
        }
        let mut won = 0;
        for t in tasks {
            if t.await.expect("joined") {
                won += 1;
            }
        }
        assert_eq!(won, 1, "eight racing reserves, one row");
        let held = tenants::own_of(&h.pool, "ada")
            .await
            .expect("own_of")
            .expect("she holds one");
        assert_eq!(held.state, State::Provisioning);
        assert_eq!(held.own_of.as_deref(), Some("ada"));

        // Tenants made on somebody's say-so carry no `own_of`, and any
        // number of them may exist.
        for id in ["spc_plain1", "spc_plain2"] {
            assert!(tenants::reserve(&h.pool, id, "Plain", "ada", 1_000, None)
                .await
                .expect("reserve"));
        }

        // A failed one is outside the index.
        tenants::fail(&h.pool, &held.id, "test", 1_001)
            .await
            .expect("failed");
        assert!(tenants::own_of(&h.pool, "ada")
            .await
            .expect("own_of")
            .is_none());
        assert!(
            tenants::reserve(&h.pool, "spc_again", "Again", "ada", 1_002, Some("ada"))
                .await
                .expect("reserve"),
            "a failed own tenant does not hold the latch"
        );
    }

    /// One person, one own tenant, however many times it is asked: eight
    /// concurrent `provision_own` calls for one principal end with one
    /// `active` tenant, one database, and every caller told which.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_person_gets_one_own_tenant_however_many_times_it_is_asked() {
        let _serial = crate::SERIAL.lock().await;
        let h = harness("ordo_pown_one").await;
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let ops = h.ops.clone();
            tasks.push(tokio::spawn(async move {
                ops.provision_own("ada_lovelace", "Ada Lovelace").await
            }));
        }
        let mut answers = Vec::new();
        for t in tasks {
            answers.push(t.await.expect("joined").expect("every ask is answered"));
        }
        let created: Vec<&OwnTenant> = answers
            .iter()
            .filter(|a| matches!(a, OwnTenant::Created { .. }))
            .collect();
        assert_eq!(created.len(), 1, "one call made it: {answers:?}");
        let space = created[0].space().to_string();
        for a in &answers {
            assert_eq!(a.space(), space, "every caller is told the one tenant");
        }

        let rows = own_rows(&h.pool, "ada_lovelace").await;
        assert_eq!(rows, vec![(space.clone(), "active".to_string())]);
        assert_eq!(tenants::all(&h.pool).await.expect("all").len(), 1);
        assert!(database_exists(&h.pool, &space).await);

        // And asked again later, it is the same.
        let again = h
            .ops
            .provision_own("ada_lovelace", "Ada")
            .await
            .expect("again");
        assert_eq!(
            again,
            OwnTenant::Existing {
                space: space.clone()
            }
        );
        assert_eq!(own_rows(&h.pool, "ada_lovelace").await.len(), 1);

        // A different person is a different tenant.
        let other = h
            .ops
            .provision_own("grace_hopper", "")
            .await
            .expect("grace");
        assert_ne!(other.space(), space);
        assert!(matches!(other, OwnTenant::Created { .. }));

        drop_database(&space);
        drop_database(other.space());
    }

    /// A tenant being built by somebody else is reported as such and nothing
    /// is made; when that row fails, the next attempt provisions.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_own_tenant_does_not_block_the_next_attempt() {
        let _serial = crate::SERIAL.lock().await;
        let h = harness("ordo_pown_failed").await;

        // A provision in flight elsewhere: a `provisioning` row.
        assert!(tenants::reserve(
            &h.pool,
            "spc_elsewhere",
            "Elsewhere",
            "ada",
            1_000,
            Some("ada")
        )
        .await
        .expect("reserve"));
        let waiting = h.ops.provision_own("ada", "Ada").await.expect("answered");
        assert_eq!(
            waiting,
            OwnTenant::InProgress {
                space: "spc_elsewhere".into()
            }
        );
        assert_eq!(tenants::all(&h.pool).await.expect("all").len(), 1);

        // A real failure: custody refuses the new space after the row exists.
        tenants::fail(&h.pool, "spc_elsewhere", "timed out", 1_001)
            .await
            .expect("timed out");
        let refusing = TenantOps::with_routing(
            h.pool.clone(),
            Arc::new(RefusesNewSpaces(h.fixture.keys.clone())),
            HOME,
            routing(&h.deployment, &h.recorder),
        );
        let err = refusing
            .provision_own("ada", "Ada")
            .await
            .expect_err("custody refused");
        assert!(err.message.contains("failed"), "{}", err.message);
        let states: Vec<String> = own_rows(&h.pool, "ada")
            .await
            .into_iter()
            .map(|(_, s)| s)
            .collect();
        assert_eq!(
            states,
            vec!["failed", "failed"],
            "both visible, neither live"
        );
        assert!(tenants::own_of(&h.pool, "ada")
            .await
            .expect("own_of")
            .is_none());

        // The next attempt, with custody working, provisions.
        let made = h
            .ops
            .provision_own("ada", "Ada")
            .await
            .expect("next attempt");
        assert!(matches!(made, OwnTenant::Created { .. }), "{made:?}");
        let live = tenants::own_of(&h.pool, "ada")
            .await
            .expect("own_of")
            .expect("one live");
        assert_eq!(live.id, made.space());
        assert_eq!(live.state, State::Active);
        assert_eq!(own_rows(&h.pool, "ada").await.len(), 3);

        drop_database(made.space());
    }

    /// What a person's own tenant is: the serving schema version, a database
    /// of its own, and the person as its one owner, attributed to them. The
    /// wall time of one provision is printed (run with `--nocapture`).
    #[tokio::test(flavor = "multi_thread")]
    async fn an_own_tenant_has_the_serving_schema_a_database_of_its_own_and_its_person_as_owner() {
        let _serial = crate::SERIAL.lock().await;
        let h = harness("ordo_pown_shape").await;
        let began = std::time::Instant::now();
        let made = h
            .ops
            .provision_own("ada_lovelace", "Ada Lovelace")
            .await
            .expect("provisioned");
        eprintln!(
            "provision_own wall time, one tenant, local Postgres: {:?}",
            began.elapsed()
        );
        let OwnTenant::Created { space, reachable } = made else {
            panic!("created: {made:?}");
        };
        assert!(reachable, "announced to the replicas");

        // A database of its own, named by the id, not the control database.
        let attachment = ordo_exec::spacedb::lookup(&h.pool, &ordo_crypto::SpaceId(space.clone()))
            .await
            .expect("$spacedb")
            .expect("every tenant has a database");
        assert_eq!(attachment.db, space);
        assert_eq!(attachment.attached_by, ordo_host::provision::SYSTEM_ACTOR);
        assert_eq!(
            attachment.custody,
            ordo_exec::spacedb::DbCustody::PlatformManaged
        );

        // The serving schema, at the serving version.
        let theirs = Pool::connect(PoolConfig {
            db: space.clone(),
            size: 2,
            ..PoolConfig::from_env()
        })
        .await
        .expect("the tenant's own database");
        assert_eq!(
            ordo_exec::spacedb::recorded_version(&theirs)
                .await
                .expect("version"),
            ordo_exec::spacedb::TargetVersion::At(h.deployment.current().version.to_string())
        );

        // The person is the one owner, and the row is attributed to them.
        let members = theirs
            .rows(&format!(
                "SELECT \"id\", \"principal\", \"displayName\", \"role\", \"status\" \
                 FROM \"members\" WHERE \"$space\" = '{space}'"
            ))
            .await
            .expect("members");
        assert_eq!(members.len(), 1, "one member: {members:?}");
        let m: Vec<String> = members[0]
            .iter()
            .map(|c| c.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            m,
            [
                "ada_lovelace",
                "ada_lovelace",
                "Ada Lovelace",
                "owner",
                "active"
            ]
        );
        let tenant = tenants::lookup(&h.pool, &space)
            .await
            .expect("lookup")
            .expect("registered");
        assert_eq!(tenant.created_by, "ada_lovelace");
        assert_eq!(tenant.own_of.as_deref(), Some("ada_lovelace"));
        assert_eq!(tenant.name, "Ada Lovelace");
        assert_eq!(tenant.state, State::Active);

        // Recorded as an own-tenant enrolment, naming the person and the
        // tenant.
        let seen = format!(
            "{:?}",
            h.recorder
                .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        );
        assert!(seen.contains("own-tenant"), "{seen}");
        assert!(
            seen.contains(&format!("space={space} result=created")),
            "{seen}"
        );

        // With no display, the name is the principal id.
        let bare = h
            .ops
            .provision_own("grace_hopper", "  ")
            .await
            .expect("bare");
        let bare_row = tenants::lookup(&h.pool, bare.space())
            .await
            .expect("lookup")
            .expect("registered");
        assert_eq!(bare_row.name, "grace_hopper");

        // A principal the door could not address is refused, and nothing is
        // written.
        let before = tenants::all(&h.pool).await.expect("all").len();
        let refused = h
            .ops
            .provision_own("bad id", "x")
            .await
            .expect_err("refused");
        assert_eq!(refused.code, ordo_api::ErrorCode::TenantRefused);
        assert_eq!(tenants::all(&h.pool).await.expect("all").len(), before);

        drop_database(&space);
        drop_database(bare.space());
    }

    /// A key provider that reads and will not mint, so a provision fails
    /// after its registry row exists.
    struct RefusesNewSpaces(Arc<dyn ordo_crypto::KeyProvider>);

    impl ordo_crypto::KeyProvider for RefusesNewSpaces {
        fn create_space(&self, _s: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::NotLeased)
        }
        fn release(
            &self,
            s: &ordo_crypto::SpaceId,
            p: ordo_crypto::Purpose,
            c: &str,
        ) -> Result<ordo_crypto::KeyLease, ordo_crypto::KeyError> {
            self.0.release(s, p, c)
        }
        fn seal_at(
            &self,
            l: &ordo_crypto::KeyLease,
            e: &str,
            f: &str,
            a: ordo_crypto::Anchor<'_>,
            v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            self.0.seal_at(l, e, f, a, v)
        }
        fn open_at(
            &self,
            l: &ordo_crypto::KeyLease,
            e: &str,
            f: &str,
            a: ordo_crypto::Anchor<'_>,
            v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            self.0.open_at(l, e, f, a, v)
        }
        #[allow(clippy::too_many_arguments)]
        fn seal_chunk(
            &self,
            l: &ordo_crypto::KeyLease,
            e: &str,
            f: &str,
            a: ordo_crypto::Anchor<'_>,
            ep: ordo_crypto::Epoch,
            b: &ordo_crypto::NonceBase,
            c: u32,
            last: bool,
            v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            self.0.seal_chunk(l, e, f, a, ep, b, c, last, v)
        }
        #[allow(clippy::too_many_arguments)]
        fn open_chunk(
            &self,
            l: &ordo_crypto::KeyLease,
            e: &str,
            f: &str,
            a: ordo_crypto::Anchor<'_>,
            ep: ordo_crypto::Epoch,
            b: &ordo_crypto::NonceBase,
            c: u32,
            last: bool,
            v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            self.0.open_chunk(l, e, f, a, ep, b, c, last, v)
        }
        fn blind_index(
            &self,
            s: &ordo_crypto::SpaceId,
            e: &str,
            f: &str,
            v: ordo_crypto::BlindValue<'_>,
        ) -> Result<[u8; 32], ordo_crypto::KeyError> {
            self.0.blind_index(s, e, f, v)
        }
        fn current_epoch(
            &self,
            s: &ordo_crypto::SpaceId,
        ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
            self.0.current_epoch(s)
        }
        fn rotate(
            &self,
            s: &ordo_crypto::SpaceId,
            r: &str,
        ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
            self.0.rotate(s, r)
        }
        fn destroy_epoch(
            &self,
            s: &ordo_crypto::SpaceId,
            e: ordo_crypto::Epoch,
        ) -> Result<(), ordo_crypto::KeyError> {
            self.0.destroy_epoch(s, e)
        }
        fn destroy_space(&self, s: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
            self.0.destroy_space(s)
        }
    }
}

// ── P2 and P3: the setting, the hook and the limits ──────────────────────

mod signup {
    use std::net::{IpAddr, SocketAddr};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use ordo_api::BoundParams;
    use ordo_conformance::Fixture;
    use ordo_host::auth::AuthState;
    use ordo_host::self_serve::{
        Limit, SelfServe, SelfServeConfig, SelfServeOps, SelfServeStore, Signup,
    };
    use ordo_host::tenants;
    use ordo_identity::directory::{Directory, Provisioning};
    use ordo_identity::provider::{Argon2idHasher, LocalPasswordProvider};
    use ordo_identity::software_authenticator::TestAuthenticator;
    use ordo_identity::webauthn::b64url_encode;
    use ordo_identity::{
        CompleteRequest, MemoryDirectoryStore, MemoryPasskeyStore, MemorySessionRevocations,
        PasskeyProvider, ProviderRegistry, RelyingParty, TokenKey,
    };
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use super::own::{drop_database, harness, own_rows, H};

    const SECRET: &[u8] = b"provision-signup-test-secret";

    struct S {
        own: H,
        app: axum::Router,
        store: SelfServeStore,
        keys: Arc<MemoryPasskeyStore>,
        clock: Arc<AtomicU64>,
        /// The stand-in Discord the `discord` connection points at.
        fake: crate::oauth2_support::FakeDiscord,
        dir: Arc<MemoryDirectoryStore>,
        /// The `email` method: its provider, and the outbox it queues to.
        email: Arc<ordo_host::email_code::EmailCodeProvider>,
        mail: Arc<ordo_host::mail::MemoryMailQueue>,
    }

    async fn setup(db: &str, mode: Option<SelfServe>) -> S {
        let own = harness(db).await;
        ordo_host::self_serve::ensure_table(&own.pool)
            .await
            .expect("tables");
        for t in ["$deploymentsetting", "$signup"] {
            own.pool
                .rows(&format!("DELETE FROM \"{t}\""))
                .await
                .expect("clean");
        }
        let store = SelfServeStore::new(own.pool.clone());
        if let Some(m) = mode {
            store.set(Some(m), None, "test", 1).await.expect("set");
        }
        let rp = RelyingParty::new("example.com", &["https://app.example.com"]).expect("rp");
        let keys = Arc::new(MemoryPasskeyStore::new());
        let passkey = Arc::new(PasskeyProvider::with_stores(
            rp,
            keys.clone(),
            Arc::new(ordo_identity::MemoryCeremonyStore::new()),
        ));
        let password = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
        password
            .enroll("ana@example.test", "correct-horse")
            .expect("enrolled");
        let mut registry = ProviderRegistry::new();
        registry
            .register("password", Box::new(password))
            .expect("pw");
        registry
            .register("passkey", Box::new(passkey.clone()))
            .expect("pk");
        let fake = crate::oauth2_support::FakeDiscord::start().await;
        let discord = fake.provider(
            "discord",
            Arc::new(ordo_identity::MemoryCeremonyStore::new()),
        );
        registry
            .register("discord", Box::new(discord.clone()))
            .expect("discord");
        let dir = Arc::new(MemoryDirectoryStore::new());
        let mail = Arc::new(ordo_host::mail::MemoryMailQueue::default());
        let email = Arc::new(ordo_host::email_code::EmailCodeProvider::new(
            SECRET,
            ordo_host::email_code::EmailCodePolicy::default(),
            Arc::new(ordo_host::email_code::MemoryEmailCodeStore::new()),
            mail.clone(),
            dir.clone(),
            Provisioning::AutoOnboard,
        ));
        registry
            .register(
                "email",
                Box::new(ordo_host::email_code::Registered(email.clone())),
            )
            .expect("email");
        registry
            .register(
                ordo_host::auth::guest::METHOD,
                Box::new(ordo_host::auth::guest::GuestProvider::new(dir.clone())),
            )
            .expect("guest");
        let clock = Arc::new(AtomicU64::new(ordo_host::runtime::now_secs()));
        let mut state = AuthState::new(
            registry,
            Directory::with_store(Provisioning::AutoOnboard, dir.clone()),
            TokenKey::from_secret(SECRET),
        )
        .with_revocations(Arc::new(MemorySessionRevocations::new()))
        .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
        .with_passkey(passkey)
        .with_oauth2(discord)
        .with_email(email.clone())
        .with_guests(ordo_host::auth::guest::Guests::fixed(
            ordo_host::auth::guest::GuestMode::On,
        ));
        let read = clock.clone();
        state.now = Box::new(move || read.load(Ordering::SeqCst));
        state.wire_signup(Arc::new(Signup::new(store.clone(), own.ops.clone())));
        let app = ordo_host::auth::router(Some(Arc::new(state)));
        S {
            own,
            app,
            store,
            keys,
            clock,
            fake,
            dir,
            email,
            mail,
        }
    }

    /// An emailed-code sign-in through the routes: begin, the worker's
    /// compose, and the code typed back.
    async fn by_email(s: &S, address: &str) -> (StatusCode, Value) {
        use ordo_host::mail::Composer as _;
        let (status, begun) = send(
            &s.app,
            "/auth/begin",
            json!({"method": "email", "identifier": address}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{begun}");
        let now = s.clock.load(Ordering::SeqCst);
        let (job, _) = s.mail.take().pop().expect("queued");
        let msg = s
            .email
            .compose(
                &ordo_host::mail::QueuedMail {
                    id: 1,
                    kind: job.kind().to_string(),
                    to: job.to().clone(),
                    payload: job.payload().to_string(),
                    attempt: 1,
                    queued_at: now,
                },
                now,
            )
            .expect("composed")
            .expect("a message is owed");
        let text = msg.text();
        let at = text.find("code is ").expect("code") + "code is ".len();
        let code: String = text[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        send(
            &s.app,
            "/auth/complete",
            json!({"method": "email", "kind": "email_code",
                   "challenge": begun["challenge"], "binder": begun["binder"], "code": code}),
            None,
        )
        .await
    }

    /// A first sign-in by an emailed code takes the path a first passkey and
    /// a first Discord sign-in take (§2.6): counted against the ceiling once
    /// the code matched, then given a tenant by the self-serve hook.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_first_email_sign_in_runs_the_self_serve_hook() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_email", Some(SelfServe::OnSignup)).await;
        let (status, done) = by_email(&s, "ana@example.org").await;
        assert_eq!(status, StatusCode::OK, "{done}");
        assert_eq!(done["via"], "email-otp");
        let done = made(&s, done).await;
        assert!(done["space"].is_string(), "the hook ran: {done}");
        assert_eq!(count(&s, "register").await, 1, "the first sign-in counted");
        let (status, again) = by_email(&s, "ana@example.org").await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["space"], done["space"], "the same tenant");
        assert_eq!(count(&s, "register").await, 1, "a returning person is not");
    }

    /// Over the ceiling, a new address is refused with `quota_exceeded` and
    /// nobody is minted; a known one still signs in.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_first_email_sign_in_over_the_ceiling_mints_nobody() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_email_cap", Some(SelfServe::OnSignup)).await;
        s.store
            .set(None, Some(Limit::PerHour(1)), "test", 2)
            .await
            .expect("one an hour");
        let (status, first) = by_email(&s, "ana@example.org").await;
        assert_eq!(status, StatusCode::OK, "{first}");
        let (status, refused) = by_email(&s, "bo@example.org").await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
        assert_eq!(refused["error"]["code"], "quota_exceeded");
        use ordo_identity::DirectoryStore as _;
        assert_eq!(
            s.dir
                .principal_for(&ordo_identity::AuthenticatorId(
                    "email:bo@example.org".into()
                ))
                .unwrap(),
            None
        );
        let (status, back) = by_email(&s, "ana@example.org").await;
        assert_eq!(status, StatusCode::OK, "{back}");
    }

    /// A new guest through the routes.
    async fn guest(s: &S) -> (StatusCode, Value) {
        send(
            &s.app,
            "/auth/complete",
            json!({"method": "guest", "kind": "new", "display": "Guest"}),
            None,
        )
        .await
    }

    /// A new guest is a sign-up (`plan-auth-federated.md` §7A): it counts
    /// against the ceiling **whatever the self-serve mode**, since nothing
    /// else bounds it, and over the ceiling nobody is minted. A guest that
    /// comes back is not counted again.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_sign_up_ceiling_counts_guests() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_guest_cap", None).await;
        s.store
            .set(None, Some(Limit::PerHour(1)), "test", 2)
            .await
            .expect("one an hour");
        let (status, first) = guest(&s).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["space"], Value::Null, "self-serve is off: {first}");
        assert_eq!(count(&s, "register").await, 1, "the guest counted");
        let secret = first["guest_secret"].as_str().expect("secret").to_string();

        let (status, refused) = guest(&s).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
        assert_eq!(refused["error"]["code"], "quota_exceeded");
        assert!(refused.get("guest_secret").is_none());
        assert_eq!(count(&s, "register").await, 1, "nothing more was counted");

        // The guest that exists comes back, uncounted.
        let (status, back) = send(
            &s.app,
            "/auth/complete",
            json!({"method": "guest", "kind": "resume", "secret": secret}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{back}");
        assert_eq!(count(&s, "register").await, 1);
    }

    /// A first guest takes the self-serve hook like any first sign-in.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_first_guest_runs_the_self_serve_hook() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_guest", Some(SelfServe::OnSignup)).await;
        let (status, done) = guest(&s).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        assert_eq!(done["via"], "guest");
        let done = made(&s, done).await;
        assert!(done["space"].is_string(), "the hook ran: {done}");
    }

    /// A Discord sign-in through the routes, as the person whose Discord
    /// profile is `profile`.
    async fn discord(s: &S, code: &str, profile: Value) -> (StatusCode, Value) {
        crate::oauth2_support::sign_in(&s.app, &s.fake, "discord", code, profile).await
    }

    fn discord_person(id: &str) -> Value {
        let mut p = crate::oauth2_support::nelly();
        p["id"] = json!(id);
        p
    }

    /// A first sign-in through Discord takes the path a first passkey takes
    /// (`plan-auth-federated.md` §2.6): it is counted against the sign-up
    /// ceiling at the mint, and after the token the self-serve hook runs and
    /// answers its two fields. A sign-in that proves no presence is given a
    /// tenant (`a_sign_in_without_presence_gets_a_tenant`).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_first_discord_sign_in_runs_the_self_serve_hook_after_the_handoff() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_discord", Some(SelfServe::OnSignup)).await;
        let (status, done) = discord(&s, "c1", discord_person("1001")).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        assert!(done["token"].is_string(), "{done}");
        assert_eq!(done["via"], "oauth2:discord");
        let done = made(&s, done).await;
        assert!(done.get("space").is_some(), "the hook ran: {done}");
        assert!(done["space"].is_string(), "{done}");
        assert_eq!(done["provisioning"], Value::Null, "{done}");
        assert_eq!(count(&s, "register").await, 1, "the first sign-in counted");

        // A returning person is never counted again.
        let (status, again) = discord(&s, "c2", discord_person("1001")).await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(count(&s, "register").await, 1);
    }

    /// The ceiling is counted at the mint: a new Discord person over it is
    /// refused with the one `quota_exceeded`, and nobody is minted.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_first_sign_in_over_the_ceiling_mints_nobody() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_discord_cap", Some(SelfServe::OnSignup)).await;
        s.store
            .set(None, Some(Limit::PerHour(1)), "test", 2)
            .await
            .expect("one an hour");
        let (status, first) = discord(&s, "c1", discord_person("2001")).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        let (status, refused) = discord(&s, "c2", discord_person("2002")).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
        assert_eq!(refused["error"]["code"], "quota_exceeded");
        assert!(refused.get("token").is_none());
        use ordo_identity::DirectoryStore as _;
        assert_eq!(
            s.dir
                .principal_for(&ordo_identity::AuthenticatorId(
                    "oauth2:discord:2002".into()
                ))
                .unwrap(),
            None,
            "nobody was minted for the refused sign-up"
        );
        // The person already signed up is not a sign-up, and is let in.
        let (status, back) = discord(&s, "c3", discord_person("2001")).await;
        assert_eq!(status, StatusCode::OK, "{back}");
    }

    /// A sign-in's answer once the person's own tenant is made. The sign-in
    /// answers before it is (`provisioning: "in progress"`, owner 2026-10-21),
    /// so this reads `GET /auth/provisioning` until it says `ready`, and
    /// gives back the answer with the tenant's `space` in it.
    async fn made(s: &S, mut done: Value) -> Value {
        if done["provisioning"] != "in progress" {
            return done;
        }
        assert_eq!(
            done["space"],
            Value::Null,
            "no tenant before it is made: {done}"
        );
        let token = done["token"].as_str().expect("a token").to_string();
        let mut last = Value::Null;
        for _ in 0..600 {
            let req = Request::builder()
                .method("GET")
                .uri("/auth/provisioning")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("request");
            let res = s.app.clone().oneshot(req).await.expect("response");
            let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .expect("body");
            last = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            match last["state"].as_str() {
                Some("ready") => {
                    done["space"] = last["space"].clone();
                    done["provisioning"] = Value::Null;
                    return done;
                }
                Some("provisioning") => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await
                }
                _ => panic!("the tenant was not made: {last}"),
            }
        }
        panic!("the tenant was not ready in 60 seconds: {last}");
    }

    async fn send(
        app: &axum::Router,
        path: &str,
        body: Value,
        from: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        if let Some(ip) = from {
            let ip: IpAddr = ip.parse().expect("ip");
            req.extensions_mut()
                .insert(ConnectInfo(SocketAddr::new(ip, 40_000)));
        }
        let res = app.clone().oneshot(req).await.expect("response");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn registration_body(req: &CompleteRequest, display: &str) -> Value {
        let CompleteRequest::PasskeyRegistration {
            challenge,
            client_data_json,
            attestation_object,
        } = req
        else {
            panic!("not a registration");
        };
        json!({
            "method": "passkey", "kind": "passkey_registration", "challenge": challenge.0,
            "client_data_json": b64url_encode(client_data_json),
            "attestation_object": b64url_encode(attestation_object),
            "display": display,
        })
    }

    fn assertion_body(req: &CompleteRequest) -> Value {
        let CompleteRequest::PasskeyAssertion {
            challenge,
            credential_id,
            client_data_json,
            authenticator_data,
            signature,
        } = req
        else {
            panic!("not an assertion");
        };
        json!({
            "method": "passkey", "kind": "passkey_assertion", "challenge": challenge.0,
            "credential_id": b64url_encode(credential_id),
            "client_data_json": b64url_encode(client_data_json),
            "authenticator_data": b64url_encode(authenticator_data),
            "signature": b64url_encode(signature),
        })
    }

    /// A whole sign-up through the routes: begin, then the device's answer.
    async fn sign_up(s: &S, device: &TestAuthenticator, from: Option<&str>) -> (StatusCode, Value) {
        let (status, options) = send(
            &s.app,
            "/auth/register",
            json!({"method": "passkey", "display": "Ada"}),
            from,
        )
        .await;
        if status != StatusCode::OK {
            return (status, options);
        }
        let c = options["challenge"].as_str().expect("challenge");
        send(
            &s.app,
            "/auth/complete",
            registration_body(&device.register(c), "Ada"),
            from,
        )
        .await
    }

    async fn sign_in(s: &S, device: &TestAuthenticator) -> (StatusCode, Value) {
        let (_, options) = send(&s.app, "/auth/begin", json!({"method": "passkey"}), None).await;
        let c = options["challenge"].as_str().expect("challenge");
        send(
            &s.app,
            "/auth/complete",
            assertion_body(&device.assert(c)),
            None,
        )
        .await
    }

    fn principal_of(token: &str, at: u64) -> String {
        ordo_identity::verify(
            &TokenKey::from_secret(SECRET),
            token,
            ordo_identity::Timestamp(at),
        )
        .expect("a token this deployment minted")
        .principal
        .0
    }

    async fn count(s: &S, kind: &str) -> i64 {
        s.own
            .pool
            .rows(&format!(
                "SELECT count(*) FROM \"$signup\" WHERE \"kind\" = '{kind}'"
            ))
            .await
            .expect("count")[0][0]
            .as_deref()
            .expect("n")
            .parse()
            .expect("number")
    }

    /// Under `on-signup`, the first sign-in answers a token and a tenant of
    /// the person's own. The token is the person's, and the person is that
    /// tenant's one owner.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_first_sign_in_under_on_signup_answers_a_token_and_a_space_the_token_can_use() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_first", Some(SelfServe::OnSignup)).await;
        let device = TestAuthenticator::es256();
        let (status, done) = sign_up(&s, &device, None).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        let done = made(&s, done).await;
        let space = done["space"].as_str().expect("a space").to_string();
        assert_eq!(done["provisioning"], Value::Null, "{done}");
        let token = done["token"].as_str().expect("a token");
        let principal = principal_of(token, s.clock.load(Ordering::SeqCst));

        // The tenant is the person's own, active, named by their display.
        let own = tenants::own_of(&s.own.pool, &principal)
            .await
            .expect("own_of")
            .expect("the person holds one");
        assert_eq!(own.id, space);
        assert_eq!(own.state, tenants::State::Active);
        assert_eq!(own.name, "Ada");

        // And in its own database the person is the one owner, under the
        // principal the token names.
        let theirs = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
            db: space.clone(),
            size: 2,
            ..ordo_exec::PoolConfig::from_env()
        })
        .await
        .expect("their database");
        let members = theirs
            .rows(&format!(
                "SELECT \"principal\", \"role\", \"status\" FROM \"members\" \
                 WHERE \"$space\" = '{space}'"
            ))
            .await
            .expect("members");
        assert_eq!(
            members,
            vec![vec![
                Some(principal.clone()),
                Some("owner".to_string()),
                Some("active".to_string())
            ]]
        );
        drop_database(&space);
    }

    /// A second sign-in answers the same tenant and makes nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_sign_in_answers_the_same_space() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_second", Some(SelfServe::OnSignup)).await;
        let device = TestAuthenticator::es256();
        let (_, first) = sign_up(&s, &device, None).await;
        let first = made(&s, first).await;
        let space = first["space"].as_str().expect("a space").to_string();
        let (status, again) = sign_in(&s, &device).await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["space"], json!(space));
        let principal = principal_of(
            again["token"].as_str().expect("t"),
            s.clock.load(Ordering::SeqCst),
        );
        assert_eq!(own_rows(&s.own.pool, &principal).await.len(), 1);
        drop_database(&space);
    }

    /// Under `off`, the default, the answer has no tenant and none exists.
    #[tokio::test(flavor = "multi_thread")]
    async fn under_off_the_answer_has_no_space_and_no_tenant_exists() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_off", None).await;
        let device = TestAuthenticator::es256();
        let (status, done) = sign_up(&s, &device, None).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        assert_eq!(done["space"], Value::Null, "{done}");
        assert_eq!(done["provisioning"], Value::Null, "{done}");
        assert!(done["token"].is_string());
        assert!(tenants::all(&s.own.pool).await.expect("all").is_empty());
        // Off is also the stored word once written, and the same.
        s.store
            .set(Some(SelfServe::Off), None, "test", 2)
            .await
            .expect("set");
        let (_, again) = sign_in(&s, &device).await;
        assert_eq!(again["space"], Value::Null);
        assert!(tenants::all(&s.own.pool).await.expect("all").is_empty());
    }

    /// A password sign-in proves no presence. That has no bearing: which
    /// login method was used does not decide provisioning (owner,
    /// 2026-10-08), so the person gets a tenant.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sign_in_without_presence_gets_a_tenant() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_presence", Some(SelfServe::OnSignup)).await;
        let (status, done) = password_sign_in(&s).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        assert!(done["token"].is_string(), "the sign-in succeeds");
        let done = made(&s, done).await;
        let space = done["space"].as_str().expect("a tenant").to_string();
        assert_eq!(done["provisioning"], Value::Null, "{done}");
        assert_eq!(tenants::all(&s.own.pool).await.expect("all").len(), 1);
        drop_database(&space);
    }

    async fn password_sign_in(s: &S) -> (StatusCode, Value) {
        let (_, begun) = send(
            &s.app,
            "/auth/begin",
            json!({"method": "password", "identifier": "ana@example.test"}),
            None,
        )
        .await;
        send(
            &s.app,
            "/auth/complete",
            json!({"method": "password", "kind": "password",
                   "challenge": begun["challenge"], "identifier": "ana@example.test",
                   "password": "correct-horse", "display": "Ana"}),
            None,
        )
        .await
    }

    /// The op writes the setting; a later boot's environment seeds only what
    /// nothing has stored, so what the app developer set through the op is
    /// what the next boot reads.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_setting_changed_through_the_op_is_what_the_next_boot_reads() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_boot", None).await;
        let ops = SelfServeOps {
            store: s.store.clone(),
            home: super::own::HOME.to_string(),
            now: Arc::new(|| 5),
            stream: None,
        };
        let call = |pairs: &[(&str, &str)]| {
            BoundParams(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                    .collect(),
            )
        };

        // A boot that finds nothing stored is seeded by the environment.
        let env = SelfServeConfig {
            mode: Some(SelfServe::OnSignup),
            per_hour: Some(Limit::PerHour(10)),
            ..Default::default()
        };
        assert!(s.store.boot(&env, 1).await.expect("boot").is_empty());
        let now = s.store.get().await.expect("get");
        assert_eq!(now.mode, SelfServe::OnSignup);
        assert_eq!(now.per_hour, Limit::PerHour(10));

        // The op changes it, and only in the deployment's own space.
        let out = ops
            .execute(super::own::HOME, &call(&[("mode", "off")]), "owner_1")
            .await
            .expect("set");
        assert_eq!(out.rows[0]["mode"], "off");
        assert_eq!(out.rows[0]["signupsPerHour"], "10");
        let elsewhere = ops
            .execute("spc_someone", &call(&[("mode", "on-signup")]), "owner_1")
            .await
            .expect_err("only in the home space");
        assert_eq!(elsewhere.code, ordo_api::ErrorCode::NotFound);
        let bad = ops
            .execute(super::own::HOME, &call(&[("mode", "sometimes")]), "owner_1")
            .await
            .expect_err("refused");
        assert_eq!(bad.code, ordo_api::ErrorCode::TenantRefused);

        // The next boot, with the environment still saying `on-signup`,
        // finds the stored setting, names the disagreement and does not win.
        let notes = s.store.boot(&env, 9).await.expect("next boot");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains(ordo_host::self_serve::MODE_ENV),
            "{notes:?}"
        );
        let after = s.store.get().await.expect("get");
        assert_eq!(after.mode, SelfServe::Off, "the stored value wins");
        // Malformed values are refused by name.
        assert!(Limit::parse("0").is_err());
        assert!(Limit::parse("many").is_err());
        assert_eq!(Limit::parse("unlimited"), Ok(Limit::Unlimited));
    }

    // ── P3: the two limits ───────────────────────────────────────────────

    /// Sixty sign-ups an hour, by default, for the deployment. The sixty-first
    /// is refused with 429 before the ceremony begins, so nothing is minted,
    /// and an hour later the count has slid clear.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_sixty_first_sign_up_in_an_hour_is_refused_and_no_principal_is_minted() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_sixty", Some(SelfServe::OnSignup)).await;
        for n in 0..60 {
            let (status, body) =
                send(&s.app, "/auth/register", json!({"method": "passkey"}), None).await;
            assert_eq!(status, StatusCode::OK, "sign-up {n}: {body}");
        }
        let (status, body) =
            send(&s.app, "/auth/register", json!({"method": "passkey"}), None).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert_eq!(body["error"]["code"], "quota_exceeded");
        assert!(body.get("challenge").is_none(), "no ceremony began");
        assert_eq!(
            s.keys.len(),
            0,
            "no credential and so no principal was minted"
        );
        assert_eq!(
            count(&s, "register").await,
            60,
            "the refused one is not counted"
        );

        // The window slides: an hour and a second later, one may sign up.
        s.clock.fetch_add(3601, Ordering::SeqCst);
        let (status, body) =
            send(&s.app, "/auth/register", json!({"method": "passkey"}), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    /// There is no per-address limit: the sixth sign-up from one address is
    /// let through, because Ordo does not decide on a client's address.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sixth_sign_up_from_one_address_is_not_refused_for_its_address() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_address", Some(SelfServe::OnSignup)).await;
        for n in 0..8 {
            let (status, body) = send(
                &s.app,
                "/auth/register",
                json!({"method": "passkey"}),
                Some("203.0.113.7"),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "sign-up {n}: {body}");
        }
    }

    /// `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR` is proxy mode's. The
    /// sign-up settings do not read it (so even nonsense does not refuse the
    /// boot), and without proxy mode the boot prints one line saying what
    /// turns it on. Its slow-down is tested in `ordo_host::proxy`.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_per_address_setting_waits_for_proxy_mode() {
        let _serial = crate::SERIAL.lock().await;
        let var = "ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR";
        std::env::set_var(var, "nonsense");
        let cfg = ordo_host::self_serve::SelfServeConfig::from_env();
        std::env::remove_var(var);
        assert!(cfg.expect("not refused").per_hour.is_none());
        let lines = ordo_host::proxy::inert_limits(|v| v == var);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains(var) && lines[0].contains("ORDO_PROXY_HEADER"));
    }

    /// A limit is a setting, not a system value: the app developer moves
    /// both through the op, `unlimited` lifts one, and `off` runs nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_limit_is_a_setting_the_app_developer_changes() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_setting", Some(SelfServe::OnSignup)).await;
        s.store
            .set(None, Some(Limit::PerHour(2)), "test", 2)
            .await
            .expect("set");
        let register = |from: &'static str| {
            let app = s.app.clone();
            async move {
                send(
                    &app,
                    "/auth/register",
                    json!({"method": "passkey"}),
                    Some(from),
                )
                .await
                .0
            }
        };
        assert_eq!(register("203.0.113.1").await, StatusCode::OK);
        assert_eq!(register("203.0.113.1").await, StatusCode::OK);
        assert_eq!(
            register("203.0.113.1").await,
            StatusCode::TOO_MANY_REQUESTS,
            "two an hour for the deployment, as set"
        );
        s.store
            .set(None, Some(Limit::Unlimited), "test", 3)
            .await
            .expect("set");
        for _ in 0..8 {
            assert_eq!(register("203.0.113.1").await, StatusCode::OK);
        }
        // Under `off` nothing is counted at all.
        s.store
            .set(Some(SelfServe::Off), Some(Limit::PerHour(1)), "test", 4)
            .await
            .expect("set");
        let before = count(&s, "register").await;
        for _ in 0..3 {
            assert_eq!(register("203.0.113.2").await, StatusCode::OK);
        }
        assert_eq!(count(&s, "register").await, before, "off counts nothing");
    }

    /// The hook is held to the same limits. A person who registered before
    /// the setting was on holds no tenant; when the deployment's provision
    /// limit is reached their sign-in succeeds, says so, and makes nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_hook_is_limited_and_answers_limit_reached() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_hook", None).await;
        // Two people register while self-serve is off: no tenant, no count.
        let a = TestAuthenticator::es256();
        let b = TestAuthenticator {
            credential_id: b"credential-two".to_vec(),
            ..TestAuthenticator::es256()
        };
        let (status, body) = sign_up(&s, &a, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = sign_up(&s, &b, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(tenants::all(&s.own.pool).await.expect("all").is_empty());
        // On, with room for one provision an hour.
        s.store
            .set(Some(SelfServe::OnSignup), Some(Limit::PerHour(1)), "t", 2)
            .await
            .expect("set");
        let (_, first) = sign_in(&s, &a).await;
        let first = made(&s, first).await;
        let space = first["space"].as_str().expect("provisioned").to_string();
        let (status, second) = sign_in(&s, &b).await;
        assert_eq!(status, StatusCode::OK, "the sign-in still succeeds");
        assert!(second["token"].is_string());
        assert_eq!(second["space"], Value::Null, "{second}");
        assert_eq!(second["provisioning"], "limit reached", "{second}");
        assert_eq!(tenants::all(&s.own.pool).await.expect("all").len(), 1);
        // An hour later they are provisioned on the next sign-in.
        s.clock.fetch_add(3601, Ordering::SeqCst);
        let (_, later) = sign_in(&s, &b).await;
        let later = made(&s, later).await;
        let space_b = later["space"]
            .as_str()
            .expect("provisioned later")
            .to_string();
        drop_database(&space);
        drop_database(&space_b);
    }

    // ── provisioning does not wait (owner, 2026-10-21) ──────────────────

    /// `GET` or `POST /auth/provisioning` with the person's token.
    async fn provisioning(s: &S, token: &str, method: &str) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri("/auth/provisioning")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .expect("request");
        let res = s.app.clone().oneshot(req).await.expect("response");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Until the registry says `active`, or 60 seconds.
    async fn until_active(pool: &ordo_exec::Pool, space: &str) -> tenants::Tenant {
        for _ in 0..600 {
            let t = tenants::lookup(pool, space)
                .await
                .expect("lookup")
                .expect("a row");
            if t.state != tenants::State::Provisioning {
                return t;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("tenant {space} was still provisioning after 60 seconds");
    }

    /// The sign-up answers before the tenant is made: a token, no space, and
    /// `provisioning: "in progress"`. `GET /auth/provisioning` names the
    /// tenant at once and follows it to `ready`.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sign_up_answers_before_its_tenant_is_made_and_the_status_follows_it_to_ready() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_async", Some(SelfServe::OnSignup)).await;
        let device = TestAuthenticator::es256();
        let (status, done) = sign_up(&s, &device, None).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        assert_eq!(done["provisioning"], "in progress", "{done}");
        assert_eq!(done["space"], Value::Null, "{done}");
        let token = done["token"].as_str().expect("a token").to_string();
        let principal = principal_of(&token, s.clock.load(Ordering::SeqCst));
        let row = tenants::own_of(&s.own.pool, &principal)
            .await
            .expect("own_of")
            .expect("the row is written before the answer");

        let (status, first) = provisioning(&s, &token, "GET").await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert!(
            first["state"] == "provisioning" || first["state"] == "ready",
            "{first}"
        );
        assert_eq!(first["space"], json!(row.id), "{first}");

        let done = made(&s, done).await;
        assert_eq!(done["space"], json!(row.id));
        assert_eq!(
            until_active(&s.own.pool, &row.id).await.state,
            tenants::State::Active
        );
        drop_database(&row.id);
    }

    /// Two sign-ins for one person at the same moment make one tenant: the
    /// registry's latch lets one row in, and both are answered with it.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_sign_ins_for_one_person_at_once_make_one_tenant() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_twice", Some(SelfServe::OnSignup)).await;
        let ops = s.own.ops.clone();
        let (a, b) = tokio::join!(ops.start_own("ada", "Ada"), ops.start_own("ada", "Ada"));
        let (a, b) = (a.expect("first"), b.expect("second"));
        assert_eq!(a.space(), b.space(), "one tenant: {a:?} {b:?}");
        assert_eq!(own_rows(&s.own.pool, "ada").await.len(), 1);
        let t = until_active(&s.own.pool, a.space()).await;
        assert_eq!(t.state, tenants::State::Active);
        assert_eq!(own_rows(&s.own.pool, "ada").await.len(), 1);
        drop_database(a.space());
    }

    /// A process that stopped mid-provision left the row `provisioning`.
    /// At boot `resume_own` makes it again, and a second run over what the
    /// first one built finishes it: one tenant, active, one owner.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_tenant_left_part_made_is_finished_by_resume() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_resume", Some(SelfServe::OnSignup)).await;
        let ops = s.own.ops.clone();
        let space = ops
            .provision_own("grace", "Grace")
            .await
            .expect("made")
            .space()
            .to_string();
        // As if the process stopped after the database was made.
        s.own
            .pool
            .rows(&format!(
                "UPDATE \"$spaces\" SET \"state\" = 'provisioning', \"phase\" = 'database' \
                 WHERE \"$space\" = '{space}'"
            ))
            .await
            .expect("rewound");
        assert_eq!(ops.resume_own().await, 1, "one part-made tenant resumed");
        let t = until_active(&s.own.pool, &space).await;
        assert_eq!(t.state, tenants::State::Active, "{t:?}");
        assert_eq!(own_rows(&s.own.pool, "grace").await.len(), 1);
        let theirs = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
            db: space.clone(),
            size: 2,
            ..ordo_exec::PoolConfig::from_env()
        })
        .await
        .expect("their database");
        let owners = theirs
            .rows("SELECT \"principal\" FROM \"members\" WHERE \"role\" = 'owner'")
            .await
            .expect("members");
        assert_eq!(owners, vec![vec![Some("grace".to_string())]]);
        drop(theirs);
        drop_database(&space);
    }

    /// A tenant that failed is reported with a reason for the person, never
    /// the registry's own note, and `POST /auth/provisioning` makes a new one.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_tenant_is_reported_with_its_reason_and_a_retry_makes_a_new_one() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_retry", None).await;
        let device = TestAuthenticator::es256();
        let (_, done) = sign_up(&s, &device, None).await;
        let token = done["token"].as_str().expect("a token").to_string();
        let principal = principal_of(&token, s.clock.load(Ordering::SeqCst));
        let (_, off) = provisioning(&s, &token, "GET").await;
        assert_eq!(off["state"], "off", "{off}");
        assert_eq!(off["setting"], "ORDO_PROVISION_SELF_SERVE", "{off}");

        s.store
            .set(Some(SelfServe::OnSignup), None, "test", 2)
            .await
            .expect("set");
        let (_, none) = provisioning(&s, &token, "GET").await;
        assert_eq!(none["state"], "none", "{none}");

        // A tenant whose provision stopped, with a note only an operator reads.
        let failed = tenants::mint_space_id();
        assert!(
            tenants::reserve(&s.own.pool, &failed, "Ada", &principal, 1, Some(&principal))
                .await
                .expect("reserve")
        );
        tenants::fail(&s.own.pool, &failed, "dial db-7.internal:5432 refused", 2)
            .await
            .expect("fail");
        let (status, said) = provisioning(&s, &token, "GET").await;
        assert_eq!(status, StatusCode::OK, "{said}");
        assert_eq!(said["state"], "failed", "{said}");
        assert_eq!(said["space"], json!(failed));
        let reason = said["reason"].as_str().expect("a reason");
        assert!(reason.contains("before anything was made"), "{reason}");
        assert!(
            !reason.contains("db-7"),
            "the operator's note stays theirs: {reason}"
        );

        let (status, again) = provisioning(&s, &token, "POST").await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert!(
            again["state"] == "provisioning" || again["state"] == "ready",
            "{again}"
        );
        let new = again["space"].as_str().expect("a new tenant").to_string();
        assert_ne!(new, failed);
        assert_eq!(
            until_active(&s.own.pool, &new).await.state,
            tenants::State::Active
        );
        let (_, ready) = provisioning(&s, &token, "GET").await;
        assert_eq!(ready["state"], "ready", "{ready}");
        assert_eq!(ready["space"], json!(new));
        drop_database(&new);
    }

    // ── setting up an own tenant: the lease, retries, stuck (owner, 2026-10-21) ──

    /// The row a sign-up writes, as if the server that took it stopped
    /// before making the tenant. `age` seconds old.
    async fn stalled_row(s: &S, principal: &str, age: u64) -> String {
        let id = tenants::mint_space_id();
        let at = ordo_host::runtime::now_secs().saturating_sub(age);
        assert!(
            tenants::reserve(&s.own.pool, &id, "Lena", principal, at, Some(principal))
                .await
                .expect("reserve")
        );
        id
    }

    /// Two servers sweep at the same moment for one stalled setup: the lease
    /// lets one of them take it, and it is made once. While a third holder's
    /// lease is live, neither starts it.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_servers_sweeping_one_stalled_setup_make_it_once() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_lease", Some(SelfServe::OnSignup)).await;
        let other = super::own::second_server(&s.own);
        assert_ne!(other.holder(), s.own.ops.holder());
        let id = stalled_row(&s, "lena", 5).await;
        let now = ordo_host::runtime::now_secs();

        // The lease itself: two claims at once, one wins.
        let (a, b) = tokio::join!(
            tenants::claim_setup(&s.own.pool, &id, "server-a", now, 600),
            tenants::claim_setup(&s.own.pool, &id, "server-b", now, 600),
        );
        assert!(
            a.expect("a") ^ b.expect("b"),
            "exactly one holder takes the lease"
        );
        // A live lease held elsewhere: nobody starts it.
        let (a, b) = tokio::join!(s.own.ops.sweep_setups(now), other.sweep_setups(now));
        let (a, b) = (a.expect("sweep a"), b.expect("sweep b"));
        assert_eq!(a.started + b.started, 0, "{a:?} {b:?}");
        assert_eq!((a.in_progress, b.in_progress), (1, 1));

        // The holder went away: its lease is given back. Both sweep again.
        for holder in ["server-a", "server-b"] {
            tenants::release_setup(&s.own.pool, &id, holder)
                .await
                .expect("release");
        }
        let (a, b) = tokio::join!(s.own.ops.sweep_setups(now), other.sweep_setups(now));
        let (a, b) = (a.expect("sweep a"), b.expect("sweep b"));
        assert_eq!(a.started + b.started, 1, "one server runs it: {a:?} {b:?}");
        let t = until_active(&s.own.pool, &id).await;
        assert_eq!(t.state, tenants::State::Active, "{t:?}");
        assert_eq!(own_rows(&s.own.pool, "lena").await.len(), 1);
        drop_database(&id);
    }

    /// A setup that fails is never given up on: the row stays
    /// `provisioning`, the next try waits out a growing pause, and nothing is
    /// marked failed or deleted.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_setup_that_fails_is_tried_again_later_and_never_marked_failed() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_retry_later", Some(SelfServe::OnSignup)).await;
        // An id no door can bind: every try is refused before anything is
        // made.
        let id = "not a space id";
        let now = ordo_host::runtime::now_secs();
        assert!(
            tenants::reserve(&s.own.pool, id, "Carl", "carl", now, Some("carl"))
                .await
                .expect("reserve")
        );
        let pool = &s.own.pool;
        let attempts_reach = |n: u64| async move {
            for _ in 0..300 {
                let t = tenants::lookup(pool, id)
                    .await
                    .expect("lookup")
                    .expect("row");
                if t.attempts >= n {
                    return t;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            panic!("the setup was not tried {n} times");
        };
        let swept = s.own.ops.sweep_setups(now).await.expect("sweep");
        assert_eq!(swept.started, 1);
        let t = attempts_reach(1).await;
        assert_eq!(t.state, tenants::State::Provisioning, "kept: {t:?}");
        assert_eq!(t.lease_until, 0, "the lease is given back");
        let first_wait = t.retry_at.saturating_sub(now);
        assert!(
            (15..=20).contains(&first_wait),
            "the first pause is ORDO_PROVISION_RETRY_BASE_SECS: {first_wait}"
        );
        assert!(!t.note.is_empty(), "the reason is kept for the owner");

        // Not due yet: nothing starts.
        let swept = s.own.ops.sweep_setups(now).await.expect("sweep");
        assert_eq!(swept.started, 0);
        // Due: tried again, and the pause doubles. The first try's task may
        // still be letting go of its claim for a moment.
        let mut started = 0;
        for _ in 0..50 {
            started = s
                .own
                .ops
                .sweep_setups(t.retry_at)
                .await
                .expect("sweep")
                .started;
            if started > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(started, 1);
        let t2 = attempts_reach(2).await;
        assert_eq!(t2.state, tenants::State::Provisioning);
        let second_wait = t2.retry_at.saturating_sub(ordo_host::runtime::now_secs());
        assert!(
            (25..=30).contains(&second_wait),
            "the second pause doubles: {second_wait}"
        );
        // The old 15-minute reconciler leaves an own tenant alone.
        let marked = tenants::time_out_stale(&s.own.pool, 1, now + 100_000)
            .await
            .expect("reconciler");
        assert_eq!(marked, 0);
        assert_eq!(own_rows(&s.own.pool, "carl").await.len(), 1);
    }

    /// Past `ORDO_PROVISION_STUCK_SECS` a setup is reported stuck: to the
    /// person in the status, once on the security-event stream, and in the
    /// health report. Its person is refused `tenant_not_ready`, never anyone
    /// else. Once the setup runs, it is ready.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stuck_setup_is_reported_to_the_person_and_the_app_developer_and_still_finishes() {
        let _serial = crate::SERIAL.lock().await;
        let s = setup("ordo_pown_su_stuck", None).await;
        let device = TestAuthenticator::es256();
        let (_, done) = sign_up(&s, &device, None).await;
        let token = done["token"].as_str().expect("a token").to_string();
        let principal = principal_of(&token, s.clock.load(Ordering::SeqCst));
        let (_, other) = password_sign_in(&s).await;
        let stranger = other["token"].as_str().expect("a token").to_string();
        s.store
            .set(Some(SelfServe::OnSignup), None, "test", 2)
            .await
            .expect("set");

        // Six minutes old, and held by a server that is not answering.
        let id = stalled_row(&s, &principal, 360).await;
        let now = ordo_host::runtime::now_secs();
        assert!(
            tenants::claim_setup(&s.own.pool, &id, "server-gone", now, 600)
                .await
                .expect("claim")
        );

        let (status, said) = provisioning(&s, &token, "GET").await;
        assert_eq!(status, StatusCode::OK, "{said}");
        assert_eq!(said["state"], "provisioning", "{said}");
        assert_eq!(said["stuck"], true, "{said}");
        assert_eq!(said["setting"], "ORDO_PROVISION_STUCK_SECS", "{said}");
        assert!(said["reason"]
            .as_str()
            .expect("a reason")
            .contains("keeps trying"));

        // The sweep reports it once, and holds it for the door.
        let _ = s.own.recorder.drain_all(ordo_commandlog::Timestamp(now));
        let swept = s.own.ops.sweep_setups(now).await.expect("sweep");
        assert_eq!((swept.in_progress, swept.stuck, swept.started), (1, 1, 0));
        let seen = format!(
            "{:?}",
            s.own
                .recorder
                .drain_all(ordo_commandlog::Timestamp(now + 1))
        );
        assert!(seen.contains("result=stuck"), "{seen}");
        let swept = s.own.ops.sweep_setups(now).await.expect("sweep");
        assert_eq!(swept.stuck, 1);
        let again = format!(
            "{:?}",
            s.own
                .recorder
                .drain_all(ordo_commandlog::Timestamp(now + 2))
        );
        assert!(!again.contains("result=stuck"), "reported once: {again}");
        let health = tenants::setup_report(&s.own.pool, now).await;
        assert_eq!(health["in_progress"], 1, "{health}");
        assert_eq!(health["stuck"][0]["space"], json!(id), "{health}");
        assert!(!health.to_string().contains(&principal), "no person named");

        // The door: its person hears `tenant_not_ready`, a stranger the
        // uniform denial.
        use ordo_api::AccessChecker as _;
        let read = s.clock.clone();
        let door = ordo_host::runtime::SwappableAccess::new(ordo_host::IdentityAccess::sharing(
            Arc::new(ordo_identity::AccessEngine::new()),
            TokenKey::from_secret(SECRET),
            Box::new(move || read.load(Ordering::SeqCst)),
            Arc::new(ordo_host::metrics::Metrics::new()),
        ));
        door.wire_not_ready(s.own.ops.not_ready());
        assert_eq!(door.not_ready(&token, &id), Some(2));
        assert_eq!(door.not_ready(&stranger, &id), None);
        assert_eq!(door.not_ready(&token, "some_other_space"), None);

        // The server that held it is gone for good: the lease runs out, the
        // next sweep takes it, and the person's tenant is made.
        tenants::release_setup(&s.own.pool, &id, "server-gone")
            .await
            .expect("release");
        let swept = s.own.ops.sweep_setups(now).await.expect("sweep");
        assert_eq!(swept.started, 1);
        assert_eq!(
            until_active(&s.own.pool, &id).await.state,
            tenants::State::Active
        );
        let (_, ready) = provisioning(&s, &token, "GET").await;
        assert_eq!(ready["state"], "ready", "{ready}");
        assert_eq!(door.not_ready(&token, &id), None, "lifted once made");
        drop_database(&id);
    }

    // ── P4: a person's own list of tenants ───────────────────────────────

    const NOTES: &str = "\
entity notes (key: nte) {
  title: text
}

op listNotes =
  notes[id, title]
";

    fn cell(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    /// A `members` row as the door reads it: space, id, role, status,
    /// principal. An empty role is a guest seat.
    fn member_row(
        space: &str,
        id: &str,
        role: &str,
        status: &str,
        who: &str,
    ) -> Vec<Option<String>> {
        vec![
            cell(space),
            cell(id),
            if role.is_empty() { None } else { cell(role) },
            cell(status),
            cell(who),
        ]
    }

    async fn get_bearer(
        app: &axum::Router,
        path: &str,
        token: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder().method("GET").uri(path);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let res = app
            .clone()
            .oneshot(b.body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Two people, and a door that knows where each belongs. Returns the
    /// tokens, the principals and the engine's cell so a test can rebuild it.
    struct Lists {
        s: S,
        ada: (String, String),
        bo: (String, String),
        engine: Arc<std::sync::Mutex<Arc<ordo_identity::AccessEngine>>>,
        fixture: Fixture,
    }

    impl Lists {
        fn install(&self, rows: &[Vec<Option<String>>]) {
            let engine = ordo_host::engine_from_members(&self.fixture, rows).expect("engine");
            *self.engine.lock().expect("engine") = Arc::new(engine);
        }
    }

    async fn lists(db: &str) -> Lists {
        let mut s = setup(db, None).await;
        // Wire the lister over a cell the test owns.
        let engine: Arc<std::sync::Mutex<Arc<ordo_identity::AccessEngine>>> = Arc::new(
            std::sync::Mutex::new(Arc::new(ordo_identity::AccessEngine::new())),
        );
        let cell = engine.clone();
        let mut registry = ProviderRegistry::new();
        let rp = RelyingParty::new("example.com", &["https://app.example.com"]).expect("rp");
        let passkey = Arc::new(PasskeyProvider::with_stores(
            rp,
            s.keys.clone(),
            Arc::new(ordo_identity::MemoryCeremonyStore::new()),
        ));
        registry
            .register("passkey", Box::new(passkey.clone()))
            .expect("pk");
        let mut state = AuthState::new(
            registry,
            Directory::with_store(
                Provisioning::AutoOnboard,
                Arc::new(MemoryDirectoryStore::new()),
            ),
            TokenKey::from_secret(SECRET),
        )
        .with_revocations(Arc::new(MemorySessionRevocations::new()))
        .with_passkey(passkey);
        let clock = s.clock.clone();
        state.now = Box::new(move || clock.load(Ordering::SeqCst));
        state.wire_spaces(Arc::new(ordo_host::auth::spaces::TenantLister {
            engine: Arc::new(move || cell.lock().expect("engine").clone()),
            pool: Some(s.own.pool.clone()),
        }));
        s.app = ordo_host::auth::router(Some(Arc::new(state)));

        let now = s.clock.load(Ordering::SeqCst);
        let a = TestAuthenticator::es256();
        let b = TestAuthenticator {
            credential_id: b"credential-two".to_vec(),
            ..TestAuthenticator::es256()
        };
        let (_, ada) = sign_up(&s, &a, None).await;
        let (_, bo) = sign_up(&s, &b, None).await;
        let token = |v: &Value| v["token"].as_str().expect("token").to_string();
        let (ada_t, bo_t) = (token(&ada), token(&bo));
        let (ada_p, bo_p) = (principal_of(&ada_t, now), principal_of(&bo_t, now));
        Lists {
            s,
            ada: (ada_t, ada_p),
            bo: (bo_t, bo_p),
            engine,
            fixture: Fixture::from_source(NOTES, "").expect("compiles"),
        }
    }

    /// A person lists their own tenants: the one they own, the one they were
    /// admitted to and the one they are a guest in, each with how. A
    /// suspended seat is not listed, nobody else's list is reachable, and a
    /// capability token, which is not a person, is refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_person_lists_their_own_tenants_and_a_capability_token_is_refused() {
        let _serial = crate::SERIAL.lock().await;
        let l = lists("ordo_pown_p4_list").await;
        let (ada_t, ada_p) = (&l.ada.0, &l.ada.1);
        let bo_p = &l.bo.1;
        for (id, name) in [("spc_a", "Ada's notes"), ("spc_b", "The team")] {
            assert!(
                tenants::reserve(&l.s.own.pool, id, name, "someone", 1, None)
                    .await
                    .expect("reserve")
            );
            assert!(tenants::activate(&l.s.own.pool, id, 2).await.expect("on"));
        }
        l.install(&[
            member_row("spc_a", "mem_a1", "owner", "active", ada_p),
            member_row("spc_b", "mem_b1", "viewer", "active", ada_p),
            member_row("spc_c", "mem_c1", "", "active", ada_p),
            member_row("spc_d", "mem_d1", "manager", "suspended", ada_p),
            member_row("spc_e", "mem_e1", "owner", "active", bo_p),
        ]);

        let (status, list) = get_bearer(&l.s.app, "/auth/spaces", Some(ada_t)).await;
        assert_eq!(status, StatusCode::OK, "{list}");
        assert_eq!(
            list,
            json!([
                {"space": "spc_a", "name": "Ada's notes", "role": "owner", "via": "member", "state": "active"},
                {"space": "spc_b", "name": "The team", "role": "viewer", "via": "member", "state": "active"},
                {"space": "spc_c", "name": null, "role": null, "via": "guest", "state": "active"},
            ])
        );

        // Bo's list is Bo's, and nothing of Ada's is in it.
        let (_, theirs) = get_bearer(&l.s.app, "/auth/spaces", Some(&l.bo.0)).await;
        assert_eq!(
            theirs,
            json!([{"space": "spc_e", "name": null, "role": "owner", "via": "member", "state": "active"}])
        );

        // No token, and a token nobody minted, are the one refusal.
        let (status, _) = get_bearer(&l.s.app, "/auth/spaces", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = get_bearer(&l.s.app, "/auth/spaces", Some("not-a-token")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // A capability token for Ada, over an op in her tenant, lists nothing.
        let engine = l.engine.lock().expect("engine").clone();
        let now = l.s.clock.load(Ordering::SeqCst);
        let grant = engine
            .grant_for(
                ordo_identity::principal::GrantId("grn_p4".into()),
                &ordo_identity::SpaceId("spc_a".into()),
                std::collections::BTreeSet::from(["listNotes".to_string()]),
                ordo_identity::Timestamp(now + 600),
            )
            .expect("listNotes is deployed");
        let capability = ordo_identity::mint(
            &TokenKey::from_secret(SECRET),
            &ordo_identity::TokenSpec {
                principal: ordo_identity::PrincipalId(ada_p.clone()),
                expires: ordo_identity::Timestamp(now + 600),
                client: None,
                user_presence_at: None,
                capability: Some(grant),
                delegation: None,
                session: None,
            },
        );
        let (status, none) = get_bearer(&l.s.app, "/auth/spaces", Some(&capability)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{none}");
        assert_eq!(none["error"]["code"], "access_denied", "{none}");
    }

    /// A tenant a person was removed from is not on their list the next
    /// time it is read.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_list_names_no_tenant_the_person_was_removed_from() {
        let _serial = crate::SERIAL.lock().await;
        let l = lists("ordo_pown_p4_removed").await;
        let ada_p = &l.ada.1;
        l.install(&[
            member_row("spc_a", "mem_a1", "owner", "active", ada_p),
            member_row("spc_b", "mem_b1", "editor", "active", ada_p),
        ]);
        let (_, before) = get_bearer(&l.s.app, "/auth/spaces", Some(&l.ada.0)).await;
        assert_eq!(before.as_array().expect("list").len(), 2, "{before}");

        // `std.space.remove` on spc_b: the door is rebuilt without the row.
        l.install(&[member_row("spc_a", "mem_a1", "owner", "active", ada_p)]);
        let (_, after) = get_bearer(&l.s.app, "/auth/spaces", Some(&l.ada.0)).await;
        let spaces: Vec<&str> = after
            .as_array()
            .expect("list")
            .iter()
            .map(|r| r["space"].as_str().expect("space"))
            .collect();
        assert_eq!(spaces, vec!["spc_a"], "{after}");
    }
}
