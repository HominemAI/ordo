//! One running `ordo-host` per nextest run, shared by an app's tests that
//! run on its default settings (owner ruling 2026-11-02, asks 3 and 6 on
//! board task `test-in-proportion-to-the-change`).
//!
//! nextest runs every test in a process of its own, so a `static` cannot
//! hold a server across tests. The first test of a run that asks for an
//! app's instance boots it. Every other test waits for its address in a
//! file named for the run. Each test isolates itself by signing up its own
//! people, who get tenants of their own.
//!
//! - **Who boots.** The first test to create the claim file. A test that
//!   finds the claim taken waits up to 3 minutes for the address, or for
//!   the booting test to say it failed.
//! - **How long it lives.** It runs under a small shell watchdog in a
//!   process group of its own, so it outlives the test that booted it. The
//!   watchdog stops it when the nextest process that ran the tests exits
//!   (nextest has no teardown step), or after 4 hours, and then drops its
//!   tenants' databases and its own.
//! - **When there is none.** Outside nextest (`cargo test`), or with
//!   `ORDO_SHARED_INSTANCE=0`, [`claim`] answers [`Claim::Own`] and each
//!   test boots its own, as before.
//!
//! A test that restarts the server, changes its settings, runs a replica,
//! applies a program or counts connections must keep booting its own.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Where a shared instance is reached.
#[derive(Debug, Clone)]
pub struct Instance {
    pub port: u16,
    /// Its control database.
    pub db: String,
    /// The file it writes its log to. Every test on it writes here.
    pub log: PathBuf,
}

/// What a test should do for an app's instance.
pub enum Claim {
    /// Not under nextest, or turned off: boot your own.
    Own,
    /// The instance is up. Use it.
    Ready(Instance),
    /// This test boots it, then calls [`Booting::publish`]. Dropping it
    /// unpublished, as a panic does, tells the waiting tests it failed.
    Boot(Booting),
}

/// The right to boot an app's shared instance for this run.
pub struct Booting {
    dir: PathBuf,
    /// The control database to boot it on.
    pub db: String,
    /// The log file to give it.
    pub log: PathBuf,
    published: bool,
}

impl Booting {
    /// Tell every waiting test where the instance is.
    pub fn publish(mut self, port: u16) -> Instance {
        let i = Instance {
            port,
            db: self.db.clone(),
            log: self.log.clone(),
        };
        let tmp = self.dir.join("addr.tmp");
        std::fs::write(&tmp, format!("{port}\n{}\n{}\n", i.db, i.log.display()))
            .expect("write the shared instance's address");
        std::fs::rename(&tmp, self.dir.join("addr")).expect("publish the address");
        self.published = true;
        i
    }
}

impl Drop for Booting {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::write(self.dir.join("failed"), "the booting test failed");
        }
    }
}

/// A tag unique to this test process. People on a shared instance must not
/// collide with another test's: a passkey's credential id carries it.
pub fn process_tag() -> &'static str {
    static TAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TAG.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("{}-{nanos:x}", std::process::id())
    })
}

/// The nextest run this process belongs to, if any.
fn run_key() -> Option<String> {
    if std::env::var("ORDO_SHARED_INSTANCE").as_deref() == Ok("0") {
        return None;
    }
    if let Ok(id) = std::env::var("NEXTEST_RUN_ID") {
        return Some(id);
    }
    // Older nextest: the run is the nextest process, our parent.
    (std::env::var("NEXTEST").as_deref() == Ok("1"))
        .then(|| format!("ppid{}", std::os::unix::process::parent_id()))
}

/// Find, or claim the right to boot, `app`'s instance for this run.
pub fn claim(app: &str) -> Claim {
    let Some(run) = run_key() else {
        return Claim::Own;
    };
    let dir = std::env::temp_dir().join(format!("ordo-shared-{app}-{run}"));
    std::fs::create_dir_all(&dir).expect("the shared instance's directory");
    if let Some(i) = read_addr(&dir) {
        return Claim::Ready(i);
    }
    let first = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("claim"))
        .is_ok();
    if first {
        // `<stem>_<pid>_<hex>` under nextest's own pid: alive while the
        // run lasts, swept by `scripts/reap-test-dbs.sh` after.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let db = format!(
            "ordo_{app}_shared_{}_{nanos:x}",
            std::os::unix::process::parent_id()
        );
        let log = dir.join("server.log");
        return Claim::Boot(Booting {
            dir,
            db,
            log,
            published: false,
        });
    }
    let started = Instant::now();
    loop {
        if let Some(i) = read_addr(&dir) {
            return Claim::Ready(i);
        }
        assert!(
            !dir.join("failed").exists(),
            "the test booting the shared {app} instance failed; see its output"
        );
        assert!(
            started.elapsed() < Duration::from_secs(180),
            "the shared {app} instance was not up after 3 minutes"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn read_addr(dir: &Path) -> Option<Instance> {
    let text = std::fs::read_to_string(dir.join("addr")).ok()?;
    let mut lines = text.lines();
    Some(Instance {
        port: lines.next()?.parse().ok()?,
        db: lines.next()?.to_string(),
        log: PathBuf::from(lines.next()?),
    })
}

/// The watchdog: start the server, stop it when nextest exits, then drop
/// the tenants' databases and the control database, and remove the run's
/// claim directory. Bounded by the nextest process and by a 4-hour
/// deadline, so it cannot outlive a run.
const WATCHDOG: &str = r#"
bin=$1 parent=$2 db=$3 psql=$4 dir=$5
"$bin" &
srv=$!
trap 'kill -TERM "$srv" 2>/dev/null' TERM INT
deadline=$(( $(date +%s) + 14400 ))
while kill -0 "$parent" 2>/dev/null && kill -0 "$srv" 2>/dev/null \
  && [ "$(date +%s)" -lt "$deadline" ]; do
  sleep 1
done
kill -TERM "$srv" 2>/dev/null
i=0
while kill -0 "$srv" 2>/dev/null && [ "$i" -lt 400 ]; do
  sleep 0.1
  i=$((i + 1))
done
kill -KILL "$srv" 2>/dev/null
pg() { "$psql" -X -q -h "$ORDO_PG_HOST" -p "$ORDO_PG_PORT" -U "$ORDO_PG_USER" "$@"; }
for t in $(pg -d "$db" -At -c 'SELECT "$space" FROM "$spaces"' 2>/dev/null); do
  pg -d postgres -c "DROP DATABASE IF EXISTS \"$t\" WITH (FORCE)" >/dev/null 2>&1
done
pg -d postgres -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" >/dev/null 2>&1
rm -rf "$dir"
"#;

/// The command that runs `bin` as a shared instance on `db`. The caller
/// sets its settings and its output as for any server, then spawns it.
/// `ORDO_PG_HOST`, `ORDO_PG_PORT` and `ORDO_PG_USER` must be set on it:
/// the watchdog reads them to drop the databases.
pub fn server_command(bin: &str, booting: &Booting) -> Command {
    let db = &booting.db;
    let dir = &booting.dir;
    use std::os::unix::process::CommandExt as _;
    let psql = ordo_conformance::Pg::default().psql;
    let mut c = Command::new("sh");
    c.arg("-c")
        .arg(WATCHDOG)
        .arg("sh")
        .arg(bin)
        .arg(std::os::unix::process::parent_id().to_string())
        .arg(db)
        .arg(psql)
        .arg(dir)
        .stdin(std::process::Stdio::null())
        // Its own process group: nextest stops a timed-out test's group,
        // and the instance is not that test's.
        .process_group(0);
    c
}
