//! **A tenant with a database of its own, moved back across B10, finds
//! every row by `==`** (increment 13; the gap PR 80 left).
//!
//! B10 computes a sealed indexed field's blind index again under a new
//! `normalize` and keeps the old one as `<field>$bidx$prev`, current under
//! every write. PR 80 traded it back in only on the deployment's own
//! database, at an activation. A tenant with its own database moves by its
//! pin. Now the node trades the kept index in before it serves the tenant
//! on a version (`Spaces::resolve`), whatever moved the pin.
//!
//! Each test: 1.0.0 declares `body` sealed and indexed, no normalization;
//! 1.1.0 adds `normalize(lower)`. Notes in mixed case are written under
//! 1.0.0, the tenant is replayed across B10 (`ordo evolve --tenant`), more
//! are written under 1.1.0, then one path moves it back. Under 1.0.0 every
//! note is found by its exact value; forward again, in any case.
//!
//! A regression looks like a row missing from `==` after a move, or a
//! write after it landing in the wrong index.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::{KeyProvider, SpaceId, TestProvider};
use ordo_exec::route::{AttachedPools, Installed, PoolResolver, SinglePool};
use ordo_exec::spacedb::{DbAttachment, DbCustody};
use ordo_exec::witness::Witness;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, ApplyOps};
use ordo_host::deploy::{self, Deployment, Swapped};
use ordo_host::schema_apply::{self as sa, Choices, Program};
use ordo_host::spaces::Spaces;
use ordo_host::tenant_forward::CatchUp;
use ordo_host::versions::Versions;
use serde_json::{json, Value};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const OWNER: &str = "deployment-owner:ada";
const T: &str = "acme";

fn ops() -> &'static str {
    "\nop addNote($title: text, $body: text) =\n  set note { title: $title, body: $body }\n"
}

fn v1() -> String {
    "domain notes @ 1.0.0\n\nentity note {\n  title: text\n  body: text sealed indexed\n}\n\n\
     op allNotes unbounded =\n  note[title]\n"
        .to_string()
        + ops()
}

fn v2() -> String {
    "domain notes @ 1.1.0\n\nentity note {\n  title: text\n  \
     body: text sealed indexed normalize(lower)\n}\n\n\
     op allNotes unbounded =\n  note[title]\n"
        .to_string()
        + ops()
}

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 4,
        ..PoolConfig::from_env()
    }
}

async fn empty_db(db: &str) -> Pool {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("an empty database");
    Pool::connect(cfg(db)).await.expect("a pool")
}

fn attach_row(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.to_string(),
        credential: "kit".into(),
        custody: DbCustody::PlatformManaged,
        attached_at: 1,
        attached_by: "test".into(),
    }
}

struct World {
    control: Pool,
    control_db: String,
    tenant_pg: Pg,
    deployment: Arc<Deployment>,
    spaces: Arc<Spaces>,
    ops: Arc<ApplyOps>,
    keys: Arc<dyn KeyProvider>,
}

fn fixture(source: &str, keys: &Arc<dyn KeyProvider>) -> Fixture {
    let mut f = Fixture::from_source(source, "").expect("compiles");
    f.keys = keys.clone();
    f
}

impl World {
    /// The deployment at 1.0.0 with 1.1.0 stored and its step stored; `T`
    /// on a database of its own at 1.0.0; a node serving 1.0.0 with 1.1.0
    /// loaded beside it.
    async fn new(stem: &str) -> World {
        let control_db = format!("ordo_kit_{stem}_control");
        let tenant_db = format!("ordo_kit_{stem}_tenant");
        let control = empty_db(&control_db).await;
        let tenant = empty_db(&tenant_db).await;
        ordo_host::tenants::ensure_table(&control)
            .await
            .expect("$spaces");
        // `T`, and `zed` on the deployment's own database, which a rollout
        // leaves for later.
        for t in [T, "zed"] {
            assert!(
                ordo_host::tenants::reserve(&control, t, t, "dev:ada", 10, None)
                    .await
                    .expect("reserved")
            );
            ordo_host::tenants::activate(&control, t, 11)
                .await
                .expect("active");
        }
        let p1 = Program::compile(&v1()).expect("1.0.0");
        apply_op::run_program(
            &control,
            &SinglePool::new(control.clone()),
            &p1,
            vec![],
            "dev:ada",
            "t",
            true,
        )
        .await
        .expect("1.0.0 is active");
        let p2 = Program::compile(&v2()).expect("1.1.0");
        ordo_exec::registry::store(
            &control,
            &ordo_exec::registry::Artifact {
                version: p2.version.clone(),
                artifact: ordo_plan::artifact::to_bytes(&p2.doc),
                source: p2.source.clone(),
                ddl: p2.ddl.clone(),
                applied_at: ordo_commandlog::Timestamp(0),
                actor: "dev:ada".into(),
            },
        )
        .await
        .expect("1.1.0 stored");
        ordo_host::evolve::evolve(
            &control,
            "1.0.0",
            "1.1.0",
            &Choices::default(),
            None,
            OWNER,
            1,
        )
        .await
        .expect("the step stores");
        let (_, created) = sa::apply(&tenant, &p1, "test", "test").await;
        assert!(
            matches!(created, Ok(sa::Applied::Created(_))),
            "{created:?}"
        );
        ordo_exec::spacedb::ensure_table(&control)
            .await
            .expect("attachments");
        let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
        keys.create_space(&SpaceId(T.into())).expect("custody");
        ordo_exec::spacedb::attach(&control, &SpaceId(T.into()), &attach_row(&tenant_db))
            .await
            .expect("attached");

        let f1 = Arc::new(fixture(&v1(), &keys));
        let deployment = Deployment::pinned(
            "1.0.0",
            f1.clone(),
            ordo_exec::Executor {
                pool: control.clone(),
                schema: f1.schema.clone(),
                registry: f1.registry.clone(),
                keys: keys.clone(),
                space: ordo_conformance::fixture_space(),
            },
        );
        let v2 = deploy::verify_embedded_pinned(
            fixture(&v2(), &keys),
            &control,
            &cfg(&control_db),
            keys.clone(),
            ordo_conformance::fixture_space(),
            "1.1.0",
        )
        .await
        .expect("1.1.0 gates");
        assert!(matches!(deployment.install(v2), Swapped::Installed { .. }));
        let pools: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
            control.clone(),
            control.clone(),
            cfg("unused"),
            Arc::new(Installed::new().as_configured("kit")),
        ));
        let spaces = Spaces::routed(
            deployment.clone(),
            pools.clone(),
            Witness::new(control.clone()),
        );
        let ops = ApplyOps::recording(control.clone(), pools, "home", None);
        let _ = ops.backfill_keys.set(keys.clone());
        let _ = ops.spaces.set(spaces.clone());
        let w = World {
            control,
            control_db,
            tenant_pg: Pg::named(&tenant_db),
            deployment,
            spaces,
            ops,
            keys,
        };
        w.take_pin().await;
        w
    }

    /// The node swaps to `version`, as it does when the activation reaches
    /// it. The version before stays loaded while a pin names it.
    async fn swap_to(&self, version: &str) {
        if self.deployment.current().version == version {
            return;
        }
        let next = self.verified(version).await;
        assert!(matches!(
            self.deployment.swap(next),
            Swapped::Installed { .. }
        ));
    }

    /// The node loads `version` beside the active one when a pin names it,
    /// as the pin stream does.
    async fn load(&self, version: &str) {
        if self.deployment.loaded(version).is_none() {
            let next = self.verified(version).await;
            assert!(matches!(
                self.deployment.install(next),
                Swapped::Installed { .. }
            ));
        }
    }

    async fn verified(&self, version: &str) -> deploy::Verified {
        let source = if version == "1.0.0" { v1() } else { v2() };
        deploy::verify_embedded_pinned(
            fixture(&source, &self.keys),
            &self.control,
            &cfg(&self.control_db),
            self.keys.clone(),
            ordo_conformance::fixture_space(),
            version,
        )
        .await
        .expect("gates")
    }

    /// The node reads the tenant's pin again, as the pin stream makes it,
    /// and loads the version it names.
    async fn take_pin(&self) {
        let on = self.pinned().await;
        self.load(&on).await;
        self.spaces.reload_pin(T).await;
    }

    async fn door(&self, op: &str, params: Value) -> Value {
        let b = ordo_api::BoundParams(params.as_object().unwrap().clone());
        let done = ordo_host::deploy_ops::execute(&self.ops, op, "home", &b, OWNER)
            .await
            .unwrap_or_else(|e| panic!("{op}: {e:?}"));
        done.rows.first().cloned().unwrap_or(Value::Null)
    }

    async fn pinned(&self) -> String {
        Versions::on(&self.control)
            .pin_of(T)
            .await
            .expect("read")
            .and_then(|p| p.version)
            .unwrap_or_default()
    }

    async fn write(&self, title: &str, body: &str) {
        let b = match self.spaces.resolve(T).await {
            Ok(b) => b,
            Err(e) => panic!(
                "the tenant is served: {e:?}; pinned {} at the control, current {}",
                self.pinned().await,
                self.deployment.current().version
            ),
        };
        b.view
            .exec
            .write_as(
                b.view.fixture.clone(),
                b.log.clone(),
                format!(r#"addNote($title: "{title}", $body: "{body}")"#),
                "p_ada".into(),
            )
            .await
            .unwrap_or_else(|e| panic!("`{title}` written: {e}"));
    }

    /// The titles `==` finds, on the version the node serves the tenant.
    async fn found(&self, body: &str) -> (String, Vec<String>) {
        let b = self.spaces.resolve(T).await.expect("the tenant is served");
        let rows = b
            .view
            .exec
            .read_as(&format!("note(body == \"{body}\")[title]"), Some("p_ada"))
            .await
            .expect("reads");
        (
            b.live.version.clone(),
            rows.iter()
                .filter_map(|r| r["title"].as_str().map(String::from))
                .collect(),
        )
    }

    /// Mixed-case notes before the step, the tenant replayed across B10 by
    /// `ordo evolve --tenant`, and more written under 1.1.0.
    async fn across_b10(&self) {
        for (title, body) in [("a", "Mixed-A"), ("b", "mixed-b"), ("c", "MIXED-c")] {
            self.write(title, body).await;
        }
        let row = self
            .door(
                ordo_host::deploy_ops::REPLAY,
                json!({ "tenant": T, "to": "1.1.0" }),
            )
            .await;
        assert_eq!(row["pinned"]["to"], "1.1.0", "{row}");
        self.take_pin().await;
        assert_eq!(
            self.tenant_pg
                .query("SELECT count(*) FROM \"$seals\" WHERE kind = 'kept'")
                .expect("seals")[0][0]
                .as_deref(),
            Some("1"),
            "B10 kept the old index"
        );
        for (title, body) in [("d", "NeW-D"), ("e", "new-E")] {
            self.write(title, body).await;
        }
        assert_eq!(
            self.found("mixed-a").await,
            ("1.1.0".into(), vec!["a".into()])
        );
    }

    /// Under 1.0.0, every note by its exact value and none by another case;
    /// a write lands; forward again, every note in any case.
    async fn finds_every_row_back_then_forward(&self, forward: impl AsyncFnOnce()) {
        for (body, title) in [
            ("Mixed-A", "a"),
            ("mixed-b", "b"),
            ("MIXED-c", "c"),
            ("NeW-D", "d"),
            ("new-E", "e"),
        ] {
            assert_eq!(
                self.found(body).await,
                ("1.0.0".into(), vec![title.to_string()]),
                "`{body}` under 1.0.0 after the move back"
            );
        }
        assert!(
            self.found("mixed-a").await.1.is_empty(),
            "1.0.0 compares exactly"
        );
        self.write("f", "Back-F").await;
        forward().await;
        for (body, title) in [("MIXED-A", "a"), ("new-d", "d"), ("back-f", "f")] {
            assert_eq!(
                self.found(body).await,
                ("1.1.0".into(), vec![title.to_string()]),
                "`{body}` under 1.1.0 forward again"
            );
        }
    }

    /// `ordo rollback` / `std.deployment.activate` through the door, the
    /// node's swap, and the catch-up's sweep, as a node runs them.
    async fn activate(&self, version: &str) {
        self.door(
            ordo_host::deploy_ops::ACTIVATE,
            json!({ "version": version }),
        )
        .await;
        self.swap_to(version).await;
        self.sweep().await;
    }

    /// The catch-up's sweep, as its tick runs it, and the pins it moved.
    async fn sweep(&self) {
        let swept = CatchUp::new(self.spaces.clone(), self.control.clone())
            .sweep()
            .await;
        assert!(swept.held.is_empty(), "{swept:?}");
        self.take_pin().await;
    }

    /// `ordo versions pin`, and the catch-up's next tick.
    async fn pin_door(&self, version: &str) {
        let row = self
            .door(
                ordo_host::deploy_ops::PIN,
                json!({ "tenant": T, "version": version }),
            )
            .await;
        assert_eq!(row["to"], version, "{row}");
        self.take_pin().await;
        self.sweep().await;
    }

    /// A rollout to `version` that moves `T` first and waits a day for the
    /// rest, and its first turn.
    async fn rollout(&self, version: &str) {
        let r = ordo_host::rollouts::Rollouts::on(&self.control);
        let first = ordo_host::rollouts::StartOptions {
            strategy: Some(ordo_host::rollouts::Strategy::Group),
            group: Some(vec![T.to_string()]),
            canary_wait: Some(86_400),
            ..Default::default()
        };
        let now = ordo_host::runtime::now_secs();
        r.start(version, &first, OWNER, now)
            .await
            .expect("the rollout starts");
        let turn = ordo_host::rollouts::tick(&self.ops, "n", now)
            .await
            .expect("a turn");
        assert!(turn.moved.contains(&T.to_string()), "{turn:?}");
        self.take_pin().await;
    }
}

/// **`ordo rollback`** to 1.0.0 after 1.1.0 was activated: the activation
/// opens the tenant's move back, the node swaps, and the catch-up moves
/// the tenant's database and pin. Forward again the same way.
#[tokio::test(flavor = "multi_thread")]
async fn a_rollback_across_b10_moves_the_tenant_and_finds_every_row() {
    let _serial = SERIAL.lock().await;
    let w = World::new("rollback").await;
    w.across_b10().await;
    w.activate("1.1.0").await;
    w.activate("1.0.0").await;
    assert_eq!(w.pinned().await, "1.0.0", "the rollback moved the tenant");
    w.finds_every_row_back_then_forward(async || {
        w.activate("1.1.0").await;
        assert_eq!(w.pinned().await, "1.1.0", "forward again");
    })
    .await;
}

/// **A rollout moves the tenant across B10, is aborted, and `ordo
/// rollback` leaves it** (§3.7: 1.0.0 is still active, so no tenant
/// moves). `ordo versions pin` takes it back to the active 1.0.0, which the
/// catch-up records on its database. A rollout takes it forward again: the
/// stored step finds the index it would compute kept whole, and does not
/// compute it again.
#[tokio::test(flavor = "multi_thread")]
async fn a_rollout_aborted_then_a_pin_back_and_a_rollout_forward_find_every_row() {
    let _serial = SERIAL.lock().await;
    let w = World::new("rollout").await;
    for (title, body) in [("a", "Mixed-A"), ("b", "mixed-b"), ("c", "MIXED-c")] {
        w.write(title, body).await;
    }
    // The deployment's own database first, as a rollout's own space needs.
    w.door(
        ordo_host::deploy_ops::EVOLVE,
        json!({ "from": "1.0.0", "to": "1.1.0", "run": true }),
    )
    .await;
    w.rollout("1.1.0").await;
    assert_eq!(w.pinned().await, "1.1.0");
    for (title, body) in [("d", "NeW-D"), ("e", "new-E")] {
        w.write(title, body).await;
    }
    w.door(
        ordo_host::rollouts::ABORT,
        json!({ "reason": "a regression in 1.1.0" }),
    )
    .await;
    w.activate("1.0.0").await;
    assert_eq!(w.pinned().await, "1.1.0", "a rollback leaves a moved seal");
    assert_eq!(w.found("mixed-a").await, ("1.1.0".into(), vec!["a".into()]));

    w.pin_door("1.0.0").await;
    w.finds_every_row_back_then_forward(async || w.rollout("1.1.0").await)
        .await;
}

/// **A move off a revoked version**: 1.1.0 is revoked while the tenant
/// serves it, and the move takes it to the active 1.0.0.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_off_a_revoked_version_across_b10_finds_every_row() {
    let _serial = SERIAL.lock().await;
    let w = World::new("revoked").await;
    w.across_b10().await;
    w.door(
        ordo_host::deploy_ops::REVOKE,
        json!({ "version": "1.1.0", "reason": "a regression in 1.1.0" }),
    )
    .await;
    let row = w
        .door(
            ordo_host::deploy_ops::MOVE_OFF_REVOKED,
            json!({ "tenant": T }),
        )
        .await;
    assert_eq!(w.pinned().await, "1.0.0", "{row}");
    w.take_pin().await;
    w.sweep().await;
    w.finds_every_row_back_then_forward(async || {
        w.door(
            ordo_host::deploy_ops::UNREVOKE,
            json!({ "version": "1.1.0", "reason": "fixed" }),
        )
        .await;
        w.pin_door("1.1.0").await;
    })
    .await;
}

/// **`ordo versions pin`** back to 1.0.0 while 1.1.0 is active, and
/// forward again.
#[tokio::test(flavor = "multi_thread")]
async fn a_pin_back_across_b10_finds_every_row() {
    let _serial = SERIAL.lock().await;
    let w = World::new("pin").await;
    w.across_b10().await;
    apply_op::activate(&w.control, "1.1.0", "dev:ada")
        .await
        .expect("1.1.0 active");
    w.swap_to("1.1.0").await;
    // A node that has not read the pin yet holds the tenant under 1.1.0.
    let stale = w.spaces.resolve(T).await.expect("served on 1.1.0");
    w.pin_door("1.0.0").await;
    w.finds_every_row_back_then_forward(async || {
        // The trade raised the tenant's pin fence: the stale node writes
        // nothing into the index 1.0.0 now reads, and is told the pin moved.
        let err = stale
            .view
            .exec
            .write_as(
                stale.view.fixture.clone(),
                stale.log.clone(),
                r#"addNote($title: "late", $body: "Late")"#.into(),
                "p_ada".into(),
            )
            .await
            .expect_err("a write under the pin before is fenced");
        assert!(err.to_string().contains("pin_moved"), "{err}");
        w.pin_door("1.1.0").await;
    })
    .await;
}

/// **Refused: a database shared with a tenant pinned to the other
/// version.** One index name cannot serve both normalizations, so the
/// tenant moved back is not served, and the refusal names the other tenant
/// and the command that fixes it. Nothing is traded.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_sharing_its_database_with_the_other_version_is_refused_naming_the_fix() {
    let _serial = SERIAL.lock().await;
    let w = World::new("shared").await;
    w.across_b10().await;
    apply_op::activate(&w.control, "1.1.0", "dev:ada")
        .await
        .expect("1.1.0 active");
    w.swap_to("1.1.0").await;
    w.keys
        .create_space(&SpaceId("zed".into()))
        .expect("custody");
    ordo_exec::spacedb::attach(
        &w.control,
        &SpaceId("zed".into()),
        &attach_row(&w.tenant_pg.db),
    )
    .await
    .expect("zed on the same database");
    Versions::on(&w.control)
        .pin("zed", "1.1.0", "dev:ada", 20)
        .await
        .expect("zed on 1.1.0");
    w.door(
        ordo_host::deploy_ops::PIN,
        json!({ "tenant": T, "version": "1.0.0" }),
    )
    .await;
    w.take_pin().await;
    let Err(e) = w.spaces.resolve(T).await else {
        panic!("served over an index it does not read");
    };
    let text = e.to_string();
    assert!(
        text.contains("`zed`") && text.contains("ordo versions pin"),
        "{text}"
    );
    assert_eq!(
        w.tenant_pg
            .query(
                "SELECT coalesce(nullif(normalize, ''), 'none') FROM \"$seals\" \
                 WHERE kind = 'kept'"
            )
            .expect("seals")[0][0]
            .as_deref(),
        Some("none"),
        "nothing traded"
    );
}
