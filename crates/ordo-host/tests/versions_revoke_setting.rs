//! Revoking the version `versions.new_tenant` names is refused, naming the
//! setting (review of increment 2b, 2026-11-04). Revoked, it would leave
//! new tenants unpinned and refuse the next boot.
//!
//! A file of its own because it sets `ORDO_VERSIONS_NEW_TENANT` for the
//! whole process, and every other test reads that variable.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, ApplyRefused};
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::{NEW_TENANT_ENV, NEW_TENANT_SETTING};

const V010: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

/// `V010` with a field added, under the next number.
const V020: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]
";

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    pool
}

async fn apply(pool: &Pool, source: &str) -> Result<apply_op::Report, ApplyRefused> {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        true,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_version_new_tenants_start_on_is_not_revoked() {
    let pool = fresh("ordo_revoke_setting").await;
    apply(&pool, V010).await.expect("applies");
    apply(&pool, V020).await.expect("applies");
    std::env::set_var(NEW_TENANT_ENV, "0.1.0");

    let refused = apply_op::revoke(&pool, "0.1.0", "flawed", None, "dev:ada")
        .await
        .expect_err("refused");
    let text = format!("{refused}");
    for want in [NEW_TENANT_SETTING, NEW_TENANT_ENV, "0.1.0"] {
        assert!(text.contains(want), "`{want}` missing from: {text}");
    }

    std::env::remove_var(NEW_TENANT_ENV);
    apply_op::revoke(&pool, "0.1.0", "flawed", None, "dev:ada")
        .await
        .expect("revoked once the setting no longer names it");
}
