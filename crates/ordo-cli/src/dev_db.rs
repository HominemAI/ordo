//! `ordo dev`'s own Postgres, and the schema delta on every save.
//! `plan-dev-loop.md` D2, §4.
//!
//! # The cluster (§4.1)
//!
//! Reuses `scripts/conformance-pg.sh`'s pattern — `initdb --auth=trust`, a
//! Unix socket, `pg_ctl` — moved into Rust, with the three corrections the
//! script's own shape invites: the port is **allocated**, not fixed at
//! 55432, so two projects open at once do not collide; the cluster is
//! **owned** — [`OwnedCluster`] stops it on drop, and the data directory
//! lives under the project so a restart is warm; `--fresh` destroys it.
//!
//! `ORDO_PG_*` still overrides everything: [`DevCluster::start`] connects to
//! whatever those name instead of starting a cluster of its own the moment
//! any of them is set, so pointing `ordo dev` at an existing database stays
//! one environment variable.
//!
//! # The delta (§4.2, §4.3)
//!
//! [`classify`] reads `live_shape`, diffs it against the schema the save
//! just compiled, and reports one of three outcomes: [`Delta::NoChange`],
//! [`Delta::Apply`] or [`Delta::Reset`]. The second is §4.3's rule made
//! literal: *"`ordo dev` never runs a migration production would not run.
//! Class A goes through the production apply path unchanged."* The
//! production path is [`ordo_host::schema_apply`], and the loop calls its
//! `assess`, `triage` and `execute` exactly as `ordo migrate --apply` does,
//! skipping only the registry and the `$migrations` row, which is the
//! version-history bookkeeping §4.3 keeps as dev's one relaxation.
//!
//! So an added optional field, a constant-defaulted one, a new entity or a
//! new index is applied while the loop keeps serving, and the rows in the
//! dev database survive it. Everything the triage refuses (a transforming
//! change, a drop, a constraint over existing rows) is a reset, as before.
//! An empty database (none of the program's tables) is a reset too, because
//! a reset is the path that seeds, and the first save on a fresh cluster is
//! the one that wants `seed.sql`.
//!
//! "Offers" is read as *performs and announces*, not as a prompt this
//! headless loop would have to block on: §4.3 replaces "auto-confirm
//! destructive migrations" with "offer a reset" precisely because a reset
//! is safe enough not to need confirmation — dev data is seed data, and it
//! is regenerated. [`reset`] runs it and [`Delta::Reset::changes`] is what
//! the loop names as the reason, printed before the reset runs.
//!
//! # What a reset may drop (27p)
//!
//! A reset drops **Ordo's tables and nothing else**. Found 2026-09-30: the
//! reset ran `DROP SCHEMA public CASCADE` on whatever database `ORDO_PG_*`
//! pointed it at, so a developer sharing a database with another system
//! lost that system's tables on the first destructive save. Ruled
//! 2026-10-01: drop only the program's tables and note the rest.
//! [`classify_tables`] is the sorting, [`ResetReport::kept`] the note.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::Digest;

use ordo_exec::{migrate, Pool, PoolConfig};
use ordo_host::schema_apply as sa;
use ordo_schema::diff::Change;
use ordo_schema::{LiveShape, Schema};

/// The live schema this loop diffs against: dev serves one hardcoded
/// namespace, same as every other host suite.
const LIVE: &str = "public";

/// `seed.sql`, beside the sources, by convention (§4.3).
pub const SEED_FILE: &str = "seed.sql";

/// The `members.id` [`reset`] seeds on every full reset, and the id
/// `dev.rs` mints the printed startup token against — the two must never
/// drift apart, so this is the one place either reads it from. `mem_`
/// matches the key prefix `members`' own `key: "mem"` derives for a real
/// row; `dev` names what it is without colliding with a seed-file
/// author's own `mem_1`, `mem_2`, …
pub const DEV_PRINCIPAL_ID: &str = "mem_dev";

// ── the cluster ─────────────────────────────────────────────────────────

/// Either a cluster `ordo dev` started and owns, or nothing — because the
/// operator named an existing one and this loop must not manage it.
pub enum DevCluster {
    Owned(OwnedCluster),
    External,
}

impl DevCluster {
    /// `ORDO_PG_HOST` (or any of its siblings) present means an operator
    /// already pointed this loop at a database; absent means `ordo dev`
    /// owns the whole lifecycle.
    pub fn start(root: &Path, fresh: bool) -> Result<(DevCluster, PoolConfig), String> {
        if std::env::var("ORDO_PG_HOST").is_ok() {
            return Ok((DevCluster::External, PoolConfig::from_env()));
        }
        let owned = OwnedCluster::start(root, fresh)?;
        let cfg = owned.pool_config();
        Ok((DevCluster::Owned(owned), cfg))
    }
}

/// A cluster this process started, stopped on drop.
pub struct OwnedCluster {
    pgbin: PathBuf,
    pgdata: PathBuf,
    socket_dir: PathBuf,
    port: u16,
    user: String,
    db: String,
}

impl OwnedCluster {
    pub fn start(root: &Path, fresh: bool) -> Result<OwnedCluster, String> {
        let pgbin = pg_bin_dir()?;
        let pgdata = root.join(".ordo").join("devdb");
        if fresh && pgdata.exists() {
            std::fs::remove_dir_all(&pgdata)
                .map_err(|e| format!("--fresh: removing {}: {e}", pgdata.display()))?;
        }
        // Data directory PER PROJECT (§4.1), under the project so a
        // restart is warm. The socket directory is separate and short: a
        // Unix socket path has a ~103-byte kernel limit, and a project
        // checked out under a long path would blow it if the socket lived
        // there too — `conformance-pg.sh`'s own comment names the same
        // constraint.
        let socket_dir = short_socket_dir(root)?;
        std::fs::create_dir_all(&socket_dir)
            .map_err(|e| format!("{}: {e}", socket_dir.display()))?;

        let user = "ordo".to_string();
        let db = "ordo_dev".to_string();
        let existed = pgdata.exists();
        if !existed {
            std::fs::create_dir_all(&pgdata).map_err(|e| format!("{}: {e}", pgdata.display()))?;
            run(
                &pgbin.join("initdb"),
                &[
                    "-D",
                    &pgdata.display().to_string(),
                    "-U",
                    &user,
                    "--auth=trust",
                ],
            )?;
        }

        let port = free_port();
        run(
            &pgbin.join("pg_ctl"),
            &[
                "-D",
                &pgdata.display().to_string(),
                "-o",
                &format!("-p {port} -k {} -h ''", socket_dir.display()),
                "-l",
                &pgdata.join("server.log").display().to_string(),
                "start",
            ],
        )?;
        wait_ready(&pgbin, &socket_dir, port)?;
        if !existed {
            run(
                &pgbin.join("createdb"),
                &[
                    "-h",
                    &socket_dir.display().to_string(),
                    "-p",
                    &port.to_string(),
                    "-U",
                    &user,
                    &db,
                ],
            )?;
        }
        Ok(OwnedCluster {
            pgbin,
            pgdata,
            socket_dir,
            port,
            user,
            db,
        })
    }

    pub fn pool_config(&self) -> PoolConfig {
        PoolConfig {
            host: self.socket_dir.display().to_string(),
            port: self.port,
            user: self.user.clone(),
            db: self.db.clone(),
            password: None,
            size: 4,
            acquire_timeout: Duration::from_secs(5),
        }
    }
}

impl Drop for OwnedCluster {
    fn drop(&mut self) {
        // Own the lifecycle (§4.1): `conformance-pg.sh stop` has no
        // cleanup trap and leaves the postmaster running if the caller
        // forgets to call it. `ordo dev` never gets a second chance to.
        let _ = Command::new(self.pgbin.join("pg_ctl"))
            .args([
                "-D",
                &self.pgdata.display().to_string(),
                "-m",
                "fast",
                "stop",
            ])
            .output();
        // The socket directory is this cluster's alone (§4.1's per-project
        // separation) — nothing else can still be using it once the
        // postmaster above has stopped.
        let _ = std::fs::remove_dir_all(&self.socket_dir);
    }
}

/// `ORDO_PGBIN`, then the path `conformance-pg.sh` defaults to, then bare
/// names on `PATH` — so a machine without the Homebrew layout still works
/// as long as the client tools are installed.
fn pg_bin_dir() -> Result<PathBuf, String> {
    // An `ORDO_PGBIN` that holds no `initdb` is a misconfiguration worth
    // naming here, rather than one that surfaces later as a failed spawn.
    if let Ok(p) = std::env::var("ORDO_PGBIN") {
        let dir = PathBuf::from(p);
        if !dir.join("initdb").exists() {
            return Err(format!(
                "no `initdb` found: ORDO_PGBIN is {} and holds none",
                dir.display()
            ));
        }
        return Ok(dir);
    }
    let homebrew = PathBuf::from("/opt/homebrew/opt/postgresql@17/bin");
    if homebrew.join("initdb").exists() {
        return Ok(homebrew);
    }
    if which("initdb") {
        return Ok(PathBuf::new());
    }
    // Debian and Ubuntu keep the server binaries out of `PATH`, under a
    // per-version directory, and install only the client tools into it. A
    // developer there has `psql` and no `initdb` on `PATH` while the server
    // is installed, so the newest versioned directory is searched before
    // giving up. Newest first: a machine with 15 and 17 installed should
    // get 17, the way the Homebrew line above pins one.
    if let Some(dir) = newest_versioned_bin("/usr/lib/postgresql") {
        return Ok(dir);
    }
    Err(
        "no `initdb` found (checked ORDO_PGBIN, /opt/homebrew/opt/postgresql@17/bin, \
         /usr/lib/postgresql/*/bin, and PATH) — set ORDO_PGBIN to a Postgres bin \
         directory, or set ORDO_PG_HOST/PORT/USER/DB to point `ordo dev` at a cluster \
         you already run"
            .to_string(),
    )
}

/// The newest `<root>/<version>/bin` that holds an `initdb`, if any.
fn newest_versioned_bin(root: &str) -> Option<PathBuf> {
    let mut found: Vec<(u32, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let bin = e.path().join("bin");
            let v: u32 = e.file_name().to_string_lossy().parse().ok()?;
            bin.join("initdb").exists().then_some((v, bin))
        })
        .collect();
    found.sort_by_key(|(v, _)| *v);
    found.pop().map(|(_, bin)| bin)
}

fn which(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A short, per-project socket directory under the system temp directory,
/// named from a digest of the canonical project root so two `ordo dev`s on
/// two projects never share one, and a machine running only one gets a
/// stable path across restarts.
fn short_socket_dir(root: &Path) -> Result<PathBuf, String> {
    let canon = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let digest = sha2::Sha256::digest(canon.as_os_str().as_encoded_bytes());
    let short = digest
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok(std::env::temp_dir().join(format!("ordo-dev-{short}")))
}

/// `pg_isready`, busy-waited — `conformance-pg.sh`'s own loop, bounded so a
/// cluster that never comes up is a refusal rather than a hang.
fn wait_ready(pgbin: &Path, socket_dir: &Path, port: u16) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let ok = Command::new(pgbin.join("pg_isready"))
            .args([
                "-h",
                &socket_dir.display().to_string(),
                "-p",
                &port.to_string(),
                "-q",
            ])
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "postgres did not become ready on {}:{port} within 15s",
                socket_dir.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A free port, released just before the caller claims it — the same race
/// `ordo-host/tests/runtime.rs`'s `free_port` accepts: in practice nothing
/// else claims it in the microseconds before `pg_ctl` binds it.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(0)
}

fn run(cmd: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{}: {e}", cmd.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{} {}: {}",
            cmd.display(),
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

// ── the delta ────────────────────────────────────────────────────────────

/// What one save's schema comparison found.
pub enum Delta {
    /// The common case, and it must cost a query and nothing else (§4.2):
    /// no probe, no apply.
    NoChange,
    /// Every change is one the production apply path sends
    /// (`ordo_host::schema_apply::triage` refused nothing): the steps, in
    /// order, for [`crate::dev::settle_delta`] to execute. The rows in the
    /// dev database survive.
    Apply {
        changes: Vec<Change>,
        steps: Vec<sa::Step>,
    },
    /// A delta the production path refuses some part of, or an empty
    /// database: drop, re-create, re-seed. `changes` is what the loop
    /// names as the reason; `refused` says which of them forced it, in the
    /// triage's own words.
    Reset {
        changes: Vec<Change>,
        refused: Vec<sa::Refused>,
    },
}

/// Read the live shape and diff it against `declared`. `prior` is the
/// schema `ordo dev` believes is live right now — `None` on the loop's
/// first save, `Some` afterward — because drops are only visible against a
/// prior declaration (`ordo_schema::diff`'s module doc): a field simply
/// absent from `declared` with no `prior` to compare against is invisible
/// to the diff, not additive.
///
/// A non-empty delta over a database that holds at least one of the
/// program's tables is assessed and triaged by the production path; one
/// over a database that holds none is a reset outright, because the reset
/// is what seeds, and no probe is worth running against no tables.
pub async fn classify(
    pool: &Pool,
    prior: Option<&Schema>,
    declared: &Schema,
) -> Result<Delta, String> {
    let live = migrate::live_shape(pool, LIVE)
        .await
        .map_err(|e| e.to_string())?;
    let changes = ordo_schema::diff(&live, prior, declared);
    if changes.is_empty() {
        return Ok(Delta::NoChange);
    }
    let present = declared
        .entities
        .iter()
        .any(|e| live.table(&e.name).is_some());
    if !present {
        return Ok(Delta::Reset {
            changes,
            refused: Vec::new(),
        });
    }
    let assessed = sa::assess(pool, declared, &live, changes.clone()).await;
    let triaged = sa::triage(pool, declared, &live, &assessed).await?;
    if triaged.refused.is_empty() {
        Ok(Delta::Apply {
            changes,
            steps: triaged.steps,
        })
    } else {
        Ok(Delta::Reset {
            changes,
            refused: triaged.refused,
        })
    }
}

/// One line per change, `code — description`, for the loop to print before
/// it resets.
pub fn describe(changes: &[Change]) -> String {
    changes
        .iter()
        .map(|c| format!("{} — {c}", c.code()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The tables a reset found in the live schema, sorted by who owns them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tables {
    /// Ordo's: dropped by the reset.
    pub ordo: Vec<String>,
    /// Another system's: left alone, and named in the report.
    pub kept: Vec<String>,
}

/// Sort the live tables into Ordo's and everyone else's (27p).
///
/// A table is Ordo's when any of these holds:
///
/// - its name begins with `$`: every platform table (`$commandlog`,
///   `$sealedpayload`, `$migrations`, …) is spelled so, and the spelling is
///   reserved to Ordo;
/// - the program declares it, now or at the loop's previous save (`prior`);
/// - it carries the `"$space"` column every entity table Ordo generates
///   carries (`ordo_schema::ddl::SPACE_COLUMN`). This is what catches a table
///   from an entity removed before a restart, when no `prior` remembers it.
///
/// Anything else was created by something other than Ordo, and a reset
/// has no business with it. The prefix-plus-identifier schema (27k) will
/// make this sorting a schema lookup; until then the three signatures
/// above are the ownership record.
pub fn classify_tables(live: &LiveShape, declared: &Schema, prior: Option<&Schema>) -> Tables {
    let declares = |s: &Schema, t: &str| s.entities.iter().any(|e| e.name == t);
    let mut out = Tables::default();
    for (name, table) in &live.tables {
        let ordo = name.starts_with('$')
            || declares(declared, name)
            || prior.is_some_and(|p| declares(p, name))
            || table.columns.contains_key(ordo_schema::ddl::SPACE_COLUMN);
        if ordo {
            out.ordo.push(name.clone());
        } else {
            out.kept.push(name.clone());
        }
    }
    out
}

/// `Fixture::reset`'s shortcut (`ordo-conformance/src/lib.rs`), over `Pool`
/// rather than a `psql` shell-out — the loop's own rule (§1.1(a)): a path
/// that runs on every schema-changing save cannot afford to fork a process
/// per statement. Drop Ordo's tables ([`classify_tables`]), apply the
/// declared DDL, then `seed.sql` beside the sources if one exists.
///
/// **Not `DROP SCHEMA public CASCADE`** (27p): that took every table in the
/// schema, another system's included. The drop is one `DROP TABLE` naming
/// every table that is Ordo's, with the default `RESTRICT`: if a table that
/// is not Ordo's depends on one that is (a foreign key, a view), Postgres
/// refuses and the error names it, and nothing of either side is touched.
/// `CASCADE` would have silently taken the other system's constraint.
///
/// `@BIDX(entity,field,value)@`/`@SPACE@` rendering (`Fixture::rendered_seed`)
/// is NOT applied to `seed.sql`: that placeholder syntax exists to make a
/// seed file space-independent under real custody, and `ordo dev`'s custody
/// story is not decided by this plan. A project whose seed inserts a
/// `sealed indexed` field's plaintext today writes the literal SQL a real
/// custody choice would need to seal instead — a gap worth closing once
/// dev's custody question is, not one this increment papers over.
pub async fn reset(
    pool: &Pool,
    root: &Path,
    declared: &Schema,
    prior: Option<&Schema>,
    ddl: &str,
) -> Result<ResetReport, String> {
    let live = migrate::live_shape(pool, LIVE)
        .await
        .map_err(|e| format!("reading the live schema: {e}"))?;
    let tables = classify_tables(&live, declared, prior);
    if !tables.ordo.is_empty() {
        let list = tables
            .ordo
            .iter()
            .map(|t| ordo_schema::ddl::q(t))
            .collect::<Vec<_>>()
            .join(", ");
        pool.rows(&format!("DROP TABLE {list}"))
            .await
            .map_err(|e| {
                format!(
                    "dropping Ordo's tables for the reset: {e}. The tables not Ordo's \
                     ({}) were left alone; if one of them depends on a table of Ordo's, \
                     remove that dependency and save again",
                    if tables.kept.is_empty() {
                        "none".to_string()
                    } else {
                        tables.kept.join(", ")
                    }
                )
            })?;
    }
    pool.rows(ddl).await.map_err(|e| e.to_string())?;
    pool.rows(ordo_crypto::SEALED_PAYLOAD_DDL)
        .await
        .map_err(|e| e.to_string())?;
    let seed_path = root.join(SEED_FILE);
    let seeded = if seed_path.exists() {
        let sql = std::fs::read_to_string(&seed_path).map_err(|e| format!("{SEED_FILE}: {e}"))?;
        if !sql.trim().is_empty() {
            pool.rows(&sql).await.map_err(|e| e.to_string())?;
        }
        true
    } else {
        false
    };

    // 17w: the development principal, seeded by `ordo dev` itself rather
    // than by the developer's own `seed.sql` — after it, so a seed file's
    // own rows are already there and nothing here depends on whether one
    // exists. `members` is the platform-baseline entity every compiled
    // schema carries (`ordo_schema::build_with_keys`), so the table above
    // always has it; `'dev'` is the same space literal
    // `dev::dev_runtime_config` hardcodes, `DEV_PRINCIPAL_ID` the same id
    // `dev::run_async` mints the printed token against — role `owner` so
    // that principal reaches any op a save declares, the same way any
    // other `ordo dev` property is meant to need no configuration.
    //
    // **Dev-only, and why this cannot leak into a real deployment:** this
    // function is called from exactly one place, `ordo_cli::dev::settle_delta`,
    // itself reachable only from `ordo dev`'s own loop
    // (`ordo_cli::dev::run_async`) — never from `ordo apply`, `ordo
    // migrate`, or the `ordo-host` server binary a real deployment runs.
    // Nothing outside the `ordo dev` subcommand calls `reset` at all, so a
    // real deployment's database never receives this row and a real
    // deployment's token secret (`ordo_identity::TokenKey::from_env`, read
    // from `ORDO_TOKEN_KEY`) never signs a token naming it.
    pool.rows(&format!(
        "INSERT INTO \"members\" (\"$space\", \"id\", \"principal\", \"displayName\", \
         \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") VALUES \
         ('dev', '{DEV_PRINCIPAL_ID}', '{DEV_PRINCIPAL_ID}', 'dev', 'principal', 1, 'owner', 'active', \
         CURRENT_DATE)"
    ))
    .await
    .map_err(|e| format!("seeding the dev principal: {e}"))?;

    Ok(ResetReport {
        seeded,
        dropped: tables.ordo.len(),
        kept: tables.kept,
    })
}

pub struct ResetReport {
    pub seeded: bool,
    /// How many of Ordo's tables the reset dropped.
    pub dropped: usize,
    /// Tables in the same schema that are not Ordo's, left as they were
    /// (27p). The loop names them so a forgotten dependency stays visible.
    pub kept: Vec<String>,
}

#[cfg(test)]
mod tests {
    //! 27p: a reset drops Ordo's tables and leaves another system's alone.
    //! Needs a Postgres cluster (`ORDO_PG_*`, `scripts/conformance-pg.sh`).

    use super::*;
    use ordo_schema::LiveColumn;

    const SRC: &str = "entity widgets {\n  name: text\n}\n";

    fn live_with(tables: &[(&str, &[&str])]) -> LiveShape {
        let mut live = LiveShape::default();
        for (table, columns) in tables {
            for column in *columns {
                live.column(
                    table,
                    column,
                    LiveColumn {
                        ty: "text".into(),
                        nullable: true,
                        has_default: false,
                    },
                );
            }
        }
        live
    }

    /// The three signatures of an Ordo table, and the absence of all three.
    #[test]
    fn classify_tables_sorts_by_ownership() {
        let (declared, _) = crate::load(SRC).expect("SRC compiles");
        let (prior, _) =
            crate::load("entity widgets {\n  name: text\n}\nentity gadgets {\n  label: text\n}\n")
                .expect("prior compiles");
        let live = live_with(&[
            ("$commandlog", &["space", "seq"]),
            ("widgets", &["$space", "id", "rev", "name"]),
            ("gadgets", &["$space", "id", "rev", "label"]),
            ("stale", &["$space", "id"]),
            ("legacy_orders", &["id", "note"]),
            ("members", &["$space", "id"]),
        ]);
        let sorted = classify_tables(&live, &declared, Some(&prior));
        assert_eq!(
            sorted.ordo,
            ["$commandlog", "gadgets", "members", "stale", "widgets"],
            "a `$` name, a declared or previously declared entity, or a `$space` column"
        );
        assert_eq!(
            sorted.kept,
            ["legacy_orders"],
            "none of the three: not Ordo's"
        );

        // With no `prior`, `gadgets` is still Ordo's: its `$space` column
        // says so even when no declaration remembers it.
        let sorted = classify_tables(&live, &declared, None);
        assert!(sorted.ordo.iter().any(|t| t == "gadgets"));
        assert_eq!(sorted.kept, ["legacy_orders"]);
    }

    /// The reset, against a real database that holds a table Ordo did not
    /// create: the foreign table and its row survive two resets, a stale
    /// Ordo table goes, and Ordo's own rows are reset.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_foreign_table_survives_a_reset_and_ordos_own_are_reset() {
        let pg = ordo_conformance::Pg::scoped("ordo_cli_dev_reset");
        pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
            .expect("fresh schema");
        let pool = Pool::connect(PoolConfig {
            db: pg.db.clone(),
            size: 2,
            ..PoolConfig::from_env()
        })
        .await
        .expect("connect to the test cluster");
        pool.ensure_platform_tables()
            .await
            .expect("platform tables");

        // Another system's table, with a row the reset must not touch.
        pool.rows(
            "CREATE TABLE legacy_orders (id int PRIMARY KEY, note text); \
             INSERT INTO legacy_orders VALUES (1, 'theirs')",
        )
        .await
        .expect("a foreign table");
        // A table from an entity this program no longer declares: Ordo's
        // by its `$space` column, and the reset owes its removal.
        pool.rows(
            "CREATE TABLE gadgets (\"$space\" text NOT NULL, id text NOT NULL, \
             rev bigint NOT NULL, label text)",
        )
        .await
        .expect("a stale Ordo table");

        let (schema, _registry) = crate::load(SRC).expect("SRC compiles");
        let ddl = ordo_schema::ddl_for(&schema).expect("ddl");
        let root = std::env::temp_dir().join(format!("ordo-dev-reset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");

        let report = reset(&pool, &root, &schema, None, &ddl)
            .await
            .expect("the first reset");
        assert_eq!(
            report.kept,
            ["legacy_orders"],
            "the foreign table is named, not dropped"
        );
        assert!(
            report.dropped >= 2,
            "gadgets and the platform tables: {}",
            report.dropped
        );

        let theirs = pool
            .rows("SELECT note FROM legacy_orders")
            .await
            .expect("the foreign table is still there");
        assert_eq!(theirs, vec![vec![Some("theirs".to_string())]]);

        let live = migrate::live_shape(&pool, LIVE).await.expect("live shape");
        assert!(live.table("gadgets").is_none(), "the stale Ordo table went");
        assert!(
            live.table("widgets").is_some(),
            "the declared table was created"
        );
        assert!(live.table("legacy_orders").is_some());

        // Ordo's own rows are reset by the next reset; the foreign row is not.
        pool.rows(
            "INSERT INTO widgets (\"$space\", id, rev, name) VALUES ('dev', 'wid_1', 1, 'mine')",
        )
        .await
        .expect("a row of Ordo's");
        let report = reset(&pool, &root, &schema, Some(&schema), &ddl)
            .await
            .expect("the second reset");
        assert_eq!(report.kept, ["legacy_orders"]);
        let mine = pool
            .rows("SELECT count(*) FROM widgets")
            .await
            .expect("count");
        assert_eq!(
            mine,
            vec![vec![Some("0".to_string())]],
            "Ordo's rows were reset"
        );
        let theirs = pool
            .rows("SELECT count(*) FROM legacy_orders")
            .await
            .expect("count");
        assert_eq!(
            theirs,
            vec![vec![Some("1".to_string())]],
            "the foreign row survived"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
