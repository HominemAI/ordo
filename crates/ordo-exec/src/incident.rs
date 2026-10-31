//! Postgres's own view of a pool incident.
//!
//! A pool that tracks its holders ([`crate::Pool::track_holders`]) can say
//! which of its callers holds a connection when it refuses one. It cannot say
//! why the holder has not let go: whether its statement is waiting on a lock
//! another connection holds, or has been answered and the answer is not being
//! read. Postgres can. This module takes one snapshot of `pg_stat_activity`
//! and `pg_locks` for the deployment's role, through a connection of its own
//! on a thread of its own, so the snapshot cannot itself wait on the pool,
//! on the pool's runtime, or on a lock (board task
//! `store-pool-held-blocks-signin`).
//!
//! What it shows of a statement is its shape only: every literal is removed
//! ([`crate::holders::redact_sql`]) and the text is cut to
//! [`crate::holders::STATEMENT_HEAD`] characters. The platform speaks the
//! simple protocol, so a statement's values are in its text.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio_postgres::{NoTls, SimpleQueryMessage};

use crate::holders::{redact_sql, STATEMENT_HEAD};
use crate::{sql_lit, PoolConfig};

/// The setting that turns the snapshot off. On by default: it runs only
/// when a tracked pool refuses a checkout, at most once per
/// [`SNAPSHOT_EVERY`], and its connection lives for one snapshot.
pub const SNAPSHOT_ENV: &str = "ORDO_PG_BUSY_SNAPSHOT";

/// The least time between two snapshots of one pool: one per incident, as
/// an incident that lasts minutes refuses many checkouts.
pub const SNAPSHOT_EVERY: Duration = Duration::from_secs(60);

/// The whole snapshot's budget: the dial, both queries and the thread.
pub const SNAPSHOT_BUDGET: Duration = Duration::from_secs(5);

/// The `application_name` a tracked pool's connections carry. Postgres keeps
/// 63 bytes of it.
pub fn application_name(pool: &str) -> String {
    let mut name = format!("ordo: {pool}");
    while name.len() > 63 {
        name.pop();
    }
    name
}

/// [`SNAPSHOT_ENV`] read: `on` (the default) or `off`. Anything else is
/// refused with the words to put in a boot refusal.
pub fn snapshot_enabled(value: Option<&str>) -> Result<bool, String> {
    match value.map(str::trim) {
        None | Some("") | Some("on") => Ok(true),
        Some("off") => Ok(false),
        Some(other) => Err(format!(
            "`{other}` is not a value of {SNAPSHOT_ENV}: say `on` (the default) or `off`"
        )),
    }
}

/// A gate that opens at most once per period.
pub struct Every {
    period: Duration,
    last: Mutex<Option<Instant>>,
}

impl Every {
    pub fn new(period: Duration) -> Every {
        Every {
            period,
            last: Mutex::new(None),
        }
    }

    /// Whether the period has passed since the gate last opened; opens it
    /// when it has.
    pub fn due(&self) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        if last.is_some_and(|at| at.elapsed() < self.period) {
            return false;
        }
        *last = Some(Instant::now());
        true
    }
}

/// Take a snapshot on a thread of its own and hand the block to `out`.
/// Returns at once.
///
/// `cfg` is the database to connect to (the deployment's control database);
/// `pool_app` is the tracked pool's `application_name`, whose backends are
/// marked `*`. The snapshot dials with `cfg.acquire_timeout` capped at 2 s
/// and a 2 s statement bound, and gives up after [`SNAPSHOT_BUDGET`].
pub fn spawn_snapshot(
    cfg: PoolConfig,
    pool_app: Option<String>,
    out: impl FnOnce(String) + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name("ordo-pool-incident".into())
        .spawn(move || {
            let block = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(async {
                    match tokio::time::timeout(SNAPSHOT_BUDGET, take(&cfg, pool_app.as_deref()))
                        .await
                    {
                        Ok(Ok(block)) => block,
                        Ok(Err(e)) => format!("pool incident: no postgres view: {e}"),
                        Err(_) => format!(
                            "pool incident: no postgres view: it took longer than {} ms",
                            SNAPSHOT_BUDGET.as_millis()
                        ),
                    }
                }),
                Err(e) => format!("pool incident: no postgres view: no runtime: {e}"),
            };
            out(block);
        });
    if let Err(e) = spawned {
        eprintln!("pool incident: no postgres view: no thread: {e}");
    }
}

const ACTIVITY_SQL: &str = r#"
WITH a AS (
  SELECT pid, datname, usename, application_name, backend_type, state,
         wait_event_type, wait_event,
         (extract(epoch FROM now() - xact_start) * 1000)::bigint AS xact_ms,
         (extract(epoch FROM now() - query_start) * 1000)::bigint AS query_ms,
         pg_blocking_pids(pid) AS blocked_by,
         left(query, 4000) AS query
    FROM pg_stat_activity
   WHERE pid <> pg_backend_pid()
)
SELECT pid, datname, application_name, backend_type, state,
       wait_event_type, wait_event, xact_ms, query_ms,
       array_to_string(blocked_by, ','), query,
       (application_name = $APP) AS mine
  FROM a
 WHERE usename = current_user
    OR pid IN (SELECT unnest(blocked_by) FROM a)
 ORDER BY (application_name = $APP) DESC, cardinality(blocked_by) DESC,
          query_ms DESC NULLS LAST
 LIMIT 60"#;

const LOCKS_SQL: &str = r#"
SELECT l.pid, l.locktype, l.mode, l.granted, d.datname,
       CASE WHEN l.relation IS NULL THEN NULL
            WHEN l.database = (SELECT oid FROM pg_database WHERE datname = current_database())
              THEN l.relation::regclass::text
            ELSE l.relation::text END,
       l.transactionid::text, l.objid::text,
       array_to_string(pg_blocking_pids(l.pid), ',')
  FROM pg_locks l LEFT JOIN pg_database d ON d.oid = l.database
 WHERE NOT l.granted
    OR l.pid IN (SELECT unnest(pg_blocking_pids(w.pid)) FROM pg_locks w WHERE NOT w.granted)
 ORDER BY l.granted, l.pid
 LIMIT 80"#;

type Row = Vec<Option<String>>;

async fn rows(client: &tokio_postgres::Client, sql: &str) -> Result<Vec<Row>, String> {
    let msgs = client
        .simple_query(sql)
        .await
        .map_err(|e| crate::driver_error(e).to_string())?;
    Ok(msgs
        .into_iter()
        .filter_map(|m| match m {
            SimpleQueryMessage::Row(r) => {
                Some((0..r.len()).map(|i| r.get(i).map(String::from)).collect())
            }
            _ => None,
        })
        .collect())
}

async fn take(cfg: &PoolConfig, pool_app: Option<&str>) -> Result<String, String> {
    let started = Instant::now();
    let bounded = PoolConfig {
        acquire_timeout: cfg.acquire_timeout.min(Duration::from_secs(2)),
        ..cfg.clone()
    };
    let (client, conn) = bounded
        .pg_named(Some(Duration::from_secs(2)), Some("ordo: pool incident"))
        .connect(NoTls)
        .await
        .map_err(|e| crate::driver_error(e).to_string())?;
    let driver = tokio::spawn(async move {
        let _ = conn.await;
    });
    let app = sql_lit(pool_app.unwrap_or(""));
    let activity = rows(&client, &ACTIVITY_SQL.replace("$APP", &app)).await?;
    let locks = rows(&client, LOCKS_SQL).await?;
    drop(client);
    driver.abort();
    Ok(render(
        pool_app,
        &activity,
        &locks,
        started.elapsed().as_millis(),
    ))
}

fn cell(r: &Row, i: usize) -> &str {
    r.get(i).and_then(|c| c.as_deref()).unwrap_or("")
}

/// The snapshot as one block, every line starting `pool incident:` so a
/// log search finds all of it.
pub fn render(pool_app: Option<&str>, activity: &[Row], locks: &[Row], took_ms: u128) -> String {
    let mut out = vec![format!(
        "pool incident: postgres view, taken in {took_ms} ms; {} backend(s), {} lock row(s); `*` marks {}",
        activity.len(),
        locks.len(),
        pool_app.unwrap_or("no pool")
    )];
    for r in activity {
        let mark = if cell(r, 11) == "t" { "*" } else { " " };
        let wait = match (cell(r, 5), cell(r, 6)) {
            ("", "") => "-".to_string(),
            (t, e) => format!("{t}/{e}"),
        };
        let ms = |i| match cell(r, i) {
            "" => "-".to_string(),
            v => format!("{v}ms"),
        };
        out.push(format!(
            "pool incident: {mark} pid={} db={} app={:?} type={} state={} wait={wait} xact={} query={} blocked_by=[{}] sql=`{}`",
            cell(r, 0),
            cell(r, 1),
            cell(r, 2),
            cell(r, 3),
            match cell(r, 4) {
                "" => "-",
                s => s,
            },
            ms(7),
            ms(8),
            cell(r, 9),
            redact_sql(cell(r, 10), STATEMENT_HEAD),
        ));
    }
    for r in locks {
        let on = [
            ("rel", cell(r, 5)),
            ("xid", cell(r, 6)),
            ("objid", cell(r, 7)),
        ]
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");
        out.push(format!(
            "pool incident:   lock pid={} {} {} {} db={} {on} blocked_by=[{}]",
            cell(r, 0),
            if cell(r, 3) == "t" {
                "holds"
            } else {
                "WAITS for"
            },
            cell(r, 2),
            cell(r, 1),
            cell(r, 4),
            cell(r, 8),
        ));
    }
    out.push("pool incident: end of postgres view".into());
    out.join("\n")
}
