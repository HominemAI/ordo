//! **The door's directory under routing**: each space's members, read from
//! the database that space lives in.
//!
//! Owner-ruled 2026-09-14 (`membership-home`): a space's members live in its
//! own database. They are entity rows that other rows reference through
//! foreign keys, and a foreign key cannot cross databases, so they sit with
//! the space. The door therefore cannot build its access engine from one
//! database any more: [`routed_engine`] reads unattached spaces' members from
//! the control database and each attached space's members from its target.
//!
//! # One source per space
//!
//! An attached space's members are read from its target **only**. A copy of
//! them left on the default database (a hand-inserted row, a provision that
//! started on the default before being attached) is dropped rather than
//! merged, so a space's membership has exactly one source and a stale row on
//! the wrong database cannot grant anything.
//!
//! # A target that cannot be read
//!
//! Its spaces get no members, so the door allows nothing in them and answers
//! a verified caller `server_busy` (a retryable 503), never `access_denied`
//! (reported fix pool-exhaustion-reads-as-denied). The deployment still
//! comes up: failing the whole build would let one tenant's database
//! take every other tenant offline. The unread targets are returned rather
//! than logged here, because what to do about one is the caller's decision
//! and "the directory is incomplete" must not be silent.
//!
//! # A rebuild reads what changed
//!
//! [`routed_engine`] reads every database. A running door keeps a [`Cache`]
//! of the last read of each one, and a rebuild for one space reads only that
//! space's database ([`Scope::Space`]). See [`Cache`] for why that is safe.

use std::collections::BTreeSet;

use ordo_conformance::Fixture;
use ordo_crypto::custody::SpaceId;
use ordo_exec::route::{target_config, DbCredentials};
use ordo_exec::spacedb::{self, DbAttachment};
use ordo_exec::{Pool, PoolConfig, PoolError, TextRow};

/// An access engine, and the targets whose members it could not read.
pub struct Directory {
    pub engine: ordo_identity::AccessEngine,
    /// One line per target that could not be read, naming the target and how
    /// many spaces live there. Empty when every database answered.
    ///
    /// **Per target**, because that is the thing that failed and an
    /// operator reading a log wants one line per failure rather than one
    /// per tenant on it.
    pub unread: Vec<String>,
    /// The same failures **per space**, for the one list `/health/ready`
    /// publishes (18d R-F1, T5).
    ///
    /// Two shapes of the same fact rather than one, and the reason is who
    /// reads them. A log line is read by a person scanning a boot, and
    /// collapsing five tenants on one downed cluster into five lines
    /// there would bury the single thing that broke. The readiness list
    /// is read per tenant, by an operator asking "is this customer
    /// served", and collapsing them there would mean a tenant whose
    /// requests are failing appears nowhere.
    ///
    /// The reason carried here is the same opaque sentence the caller
    /// gets, never the dial error, for §9.1 R-C4.4's reason: it names a
    /// host, a port and often a user.
    pub unread_spaces: Vec<(String, String)>,
    /// The spaces served from their **last good member list** because this
    /// build's read of their database failed for a passing reason and the
    /// failure is still inside the grace ([`stale_grace_from`]), as
    /// `(space, reason)`. They are not in [`Self::unread_spaces`]: the door
    /// serves them. Empty when the grace is 0 (owner, 2026-10-27).
    pub kept_stale: Vec<(String, String)>,
    /// How many tenant databases were read for this build. The others came
    /// from the [`Cache`].
    pub dialled: usize,
    /// How many tenant databases the door was built over.
    pub databases: usize,
}

/// Build the access engine from every space's own database, reading every
/// one of them. The boot's build, and what a caller without a [`Cache`]
/// gets.
pub async fn routed_engine(
    f: &Fixture,
    control: &Pool,
    credentials: &dyn DbCredentials,
    template: &PoolConfig,
) -> Result<Directory, String> {
    let reads = prefetch(control, credentials, template, Scope::All).await;
    Cache::default()
        .build(reads, f, control, credentials, template)
        .await
}

/// Which tenant databases a rebuild reads again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    /// Every one: the boot, a deploy, and the access refresh tick.
    All,
    /// Only the one this space lives in: a new tenant, or a membership
    /// change announced for this space. Every other database is taken from
    /// the [`Cache`].
    Space(&'a str),
}

/// What one tenant database held when it was read.
struct TargetRead {
    /// When the read **started**. Of two reads of one database, the one that
    /// started later wins, because it saw every change committed before it
    /// started, and a change is announced only after it commits.
    at: std::time::Instant,
    target: DbAttachment,
    /// Its `members` rows and profile rows, unfiltered, or why they could
    /// not be read.
    held: Result<(Vec<TextRow>, crate::profile_ops::TenantAccess), ReadFault>,
}

/// Why a tenant database could not be read: its text, and whether the
/// failure passes by itself ([`PoolError::is_passing`], the one place that
/// decides it). The door keeps a space's last good members only through a
/// passing failure.
#[derive(Debug, Clone)]
pub struct ReadFault {
    pub text: String,
    pub passing: bool,
}

impl ReadFault {
    fn of(e: &PoolError) -> ReadFault {
        ReadFault {
            text: e.to_string(),
            passing: e.is_passing(),
        }
    }
}

/// The setting that bounds how long a space's last good member list is
/// served while its database cannot be read for a passing reason.
pub const STALE_GRACE_SETTING: &str = "ORDO_DOOR_STALE_GRACE_SECS";
/// Its default, in seconds.
pub const STALE_GRACE_DEFAULT_SECS: u64 = 60;
/// Its ceiling, in seconds (one day). A removed member keeps access this
/// long at most, so the ceiling is a safety bound, not a recommendation.
pub const STALE_GRACE_MAX_SECS: u64 = 86_400;

/// Parse [`STALE_GRACE_SETTING`]: unset or empty is the default; `0` turns
/// the grace off (deny at once); anything else that is not a whole number of
/// seconds up to the ceiling is an error naming the setting.
pub fn stale_grace_from(raw: Option<&str>) -> Result<std::time::Duration, String> {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(std::time::Duration::from_secs(STALE_GRACE_DEFAULT_SECS));
    };
    match raw.parse::<u64>() {
        Ok(n) if n <= STALE_GRACE_MAX_SECS => Ok(std::time::Duration::from_secs(n)),
        _ => Err(format!(
            "`{raw}` is not a number of seconds from 0 to {STALE_GRACE_MAX_SECS}; 0 denies a              space at once when its members cannot be read, unset it for the default, \
             {STALE_GRACE_DEFAULT_SECS}"
        )),
    }
}

/// Reads made before taking the [`Cache`]'s lock, handed to
/// [`Cache::build`].
pub struct Reads(Vec<TargetRead>);

/// **The last read of every tenant database**, so a rebuild for one tenant
/// reads one database (reported fix slow-passkey-signup, 2026-10-21).
///
/// Before this, every rebuild dialled every tenant database in turn. A new
/// tenant rebuilds the door twice on the replica that made it (before its
/// maker is answered, and again when its announcement comes back), so each
/// sign-up cost two full passes. With 50 tenants on a cluster that asks for a
/// password, one pass took about 2 seconds on a laptop, and sign-ups queued
/// behind each other past the SDK's 15-second deadline on CI. It grew with
/// every tenant.
///
/// # Why it is safe to keep
///
/// Everything that changes a space's members or profile announces that
/// space ([`crate::member_ops::MemberOps::rebuilt`], a new tenant, a pause),
/// and the rebuild for that announcement reads that space's database again.
/// The access refresh tick still reads every database, so anything else is
/// seen within `ORDO_ACCESS_REFRESH_SECS`, the same ceiling as before.
///
/// # One rebuild at a time
///
/// The holder keeps this behind a lock from [`Cache::build`] until the door
/// it returns is installed. Without that, a rebuild that read before a
/// tenant existed could install its door after one that read the tenant, and
/// the new tenant would be refused until the next rebuild. The slow part,
/// the dials, happens in [`prefetch`], outside the lock.
pub struct Cache {
    /// By [`DbAttachment::identity`].
    targets: std::collections::HashMap<String, TargetRead>,
    /// The last **good** read of each database, kept while a passing failure
    /// is inside the grace. By [`DbAttachment::identity`].
    good: std::collections::HashMap<String, Good>,
    /// When the current run of passing failures began, per database. Gone
    /// when the database reads again or fails for a lasting reason; its
    /// presence is what makes a log line once per run.
    failing: std::collections::HashMap<String, std::time::Instant>,
    /// How long a failure run may keep serving [`Self::good`]. Zero: never.
    grace: std::time::Duration,
    /// Tests only: moves this cache's clock forward.
    skew: std::time::Duration,
    /// Tests only: every fresh read of a tenant database fails like this.
    forced: Option<ReadFault>,
}

type Held = (Vec<TextRow>, crate::profile_ops::TenantAccess);

struct Good {
    found: Vec<TextRow>,
    access: crate::profile_ops::TenantAccess,
}

impl Default for Cache {
    fn default() -> Cache {
        Cache::with_grace(std::time::Duration::from_secs(STALE_GRACE_DEFAULT_SECS))
    }
}

impl Cache {
    /// A cache that keeps a space's last good members through a passing
    /// failure for `grace`. `Duration::ZERO` denies at once.
    pub fn with_grace(grace: std::time::Duration) -> Cache {
        Cache {
            targets: Default::default(),
            good: Default::default(),
            failing: Default::default(),
            grace,
            skew: std::time::Duration::ZERO,
            forced: None,
        }
    }

    /// Tests only: fail every fresh read of a tenant database as a passing
    /// (`Some(true)`) or lasting (`Some(false)`) error, or stop doing so.
    pub fn force_failure(&mut self, passing: Option<bool>) {
        self.forced = passing.map(|passing| ReadFault {
            text: if passing {
                "deadlock detected".into()
            } else {
                "relation \"members\" does not exist".into()
            },
            passing,
        });
    }

    /// Tests only: move this cache's clock forward.
    pub fn advance(&mut self, by: std::time::Duration) {
        self.skew += by;
    }

    fn now(&self) -> std::time::Instant {
        std::time::Instant::now() + self.skew
    }

    fn force(&self, read: &mut TargetRead) {
        if let Some(f) = &self.forced {
            read.held = Err(f.clone());
        }
    }

    /// Remember `read` as the latest for its database, and as the last good
    /// one when it succeeded.
    fn take(&mut self, key: String, read: TargetRead) {
        if let Ok((found, access)) = &read.held {
            self.good.insert(
                key.clone(),
                Good {
                    found: found.clone(),
                    access: access.clone(),
                },
            );
        }
        self.targets.insert(key, read);
    }
}

async fn read_target(
    target: DbAttachment,
    credentials: &dyn DbCredentials,
    template: &PoolConfig,
) -> TargetRead {
    let at = std::time::Instant::now();
    let held = async {
        // No DDL: this runs on every rebuild, beside the writes that caused
        // it, and the platform tables' DDL deadlocks against a blob write
        // (`Pool::connect_without_ddl`). The tables were made when the
        // tenant was provisioned, and this reads none of them.
        let pool =
            Pool::connect_without_ddl(target_config(&target, credentials, template)?).await?;
        let found = pool.rows(crate::MEMBERS_SQL).await?;
        let access = crate::profile_ops::TenantAccess::read(&pool).await?;
        Ok::<Held, PoolError>((found, access))
    }
    .await
    .map_err(|e: PoolError| ReadFault::of(&e));
    TargetRead { at, target, held }
}

/// The distinct databases spaces are attached to, each with its spaces.
fn group(attached: &[(SpaceId, DbAttachment)]) -> Vec<(DbAttachment, BTreeSet<String>)> {
    let mut targets: Vec<(DbAttachment, BTreeSet<String>)> = Vec::new();
    for (space, a) in attached {
        match targets
            .iter()
            .position(|(t, _)| t.identity() == a.identity())
        {
            Some(i) => {
                targets[i].1.insert(space.0.clone());
            }
            None => targets.push((a.clone(), BTreeSet::from([space.0.clone()]))),
        }
    }
    targets
}

/// Read the tenant databases `scope` names. No lock is held, so a slow dial
/// delays only this rebuild. A failure to list the attachments reads
/// nothing here; [`Cache::build`] lists them again and reports it.
pub async fn prefetch(
    control: &Pool,
    credentials: &dyn DbCredentials,
    template: &PoolConfig,
    scope: Scope<'_>,
) -> Reads {
    let Ok(attached) = spacedb::all(control).await else {
        return Reads(Vec::new());
    };
    let mut reads = Vec::new();
    for (target, spaces) in group(&attached) {
        let wanted = match scope {
            Scope::All => true,
            Scope::Space(s) => spaces.contains(s),
        };
        if wanted {
            reads.push(read_target(target, credentials, template).await);
        }
    }
    Reads(reads)
}

impl Cache {
    /// Take in `reads`, read the control database, and build the engine.
    ///
    /// A database this cache holds no read of is read here, under the lock:
    /// the first rebuild after the boot, or a tenant attached after the
    /// reads were made.
    pub async fn build(
        &mut self,
        reads: Reads,
        f: &Fixture,
        control: &Pool,
        credentials: &dyn DbCredentials,
        template: &PoolConfig,
    ) -> Result<Directory, String> {
        let mut dialled = reads.0.len();
        for mut read in reads.0 {
            self.force(&mut read);
            let key = read.target.identity();
            let newer = self.targets.get(&key).is_none_or(|old| old.at <= read.at);
            if newer {
                self.take(key, read);
            }
        }

        spacedb::ensure_table(control)
            .await
            .map_err(|e| e.to_string())?;
        let attached = spacedb::all(control).await.map_err(|e| e.to_string())?;
        let elsewhere: BTreeSet<String> = attached.iter().map(|(s, _)| s.0.clone()).collect();

        // A database nobody has run `ordo apply` against has no `members`
        // table at all: it is a platform entity `ordo_schema` injects into
        // every program, including the empty one, so the question of whether
        // it exists is asked here rather than assumed, exactly as
        // `crate::deployment_engine_pooled` asks it of the single-database
        // path this mirrors. Without the check a fresh control database would
        // fail the boot on a missing relation instead of booting into the
        // no-program state `Deployment::empty` exists for.
        let control_applied = control
            .rows(r#"SELECT to_regclass('"members"') IS NOT NULL"#)
            .await
            .map_err(|e| e.to_string())?;
        let control_applied = matches!(
            control_applied.first().and_then(|r| r[0].as_deref()),
            Some("t") | Some("true")
        );

        let mut rows: Vec<TextRow> = if control_applied {
            control
                .rows(crate::MEMBERS_SQL)
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|r| space_of(r).is_some_and(|s| !elsewhere.contains(s)))
                .collect()
        } else {
            Vec::new()
        };
        // What each tenant changed about its profile, read from the database
        // its members are in, under the same filter.
        let mut tenant = if control_applied {
            let mut t = crate::profile_ops::TenantAccess::read(control)
                .await
                .map_err(|e| e.to_string())?;
            t.retain(|s| !elsewhere.contains(s));
            t
        } else {
            crate::profile_ops::TenantAccess::default()
        };

        // One read per distinct target, however many spaces share it.
        let targets = group(&attached);
        let live: BTreeSet<String> = targets.iter().map(|(t, _)| t.identity()).collect();
        // A database no space is attached to any more is forgotten.
        self.targets.retain(|k, _| live.contains(k));
        self.good.retain(|k, _| live.contains(k));
        self.failing.retain(|k, _| live.contains(k));

        let mut unread = Vec::new();
        let mut unread_spaces = Vec::new();
        let mut kept_stale = Vec::new();
        for (target, spaces) in targets {
            let key = target.identity();
            if !self.targets.contains_key(&key) {
                dialled += 1;
                let mut read = read_target(target.clone(), credentials, template).await;
                self.force(&mut read);
                self.take(key.clone(), read);
            }
            let held = self.targets[&key].held.clone();
            let now = self.now();
            // A run of passing failures serves the last good list until the
            // grace is spent; a lasting error, or no good list, denies.
            let held = match held {
                Ok(h) => {
                    self.failing.remove(&key);
                    Ok(h)
                }
                Err(fault) if fault.passing && !self.grace.is_zero() => {
                    let began = *self.failing.entry(key.clone()).or_insert_with(|| {
                        eprintln!(
                            "door: {}:{}/{} could not be read ({}); serving its last good \
                             member list for up to {} s while it is retried",
                            target.host,
                            target.port,
                            target.db,
                            fault.text,
                            self.grace.as_secs()
                        );
                        now
                    });
                    match self.good.get(&key) {
                        Some(g) if now.saturating_duration_since(began) < self.grace => {
                            for space in &spaces {
                                kept_stale.push((
                                    space.clone(),
                                    format!(
                                        "the database this space is served from could not be \
                                         read, so its last good member list is served for up \
                                         to {} s from {} s ago",
                                        self.grace.as_secs(),
                                        now.saturating_duration_since(began).as_secs()
                                    ),
                                ));
                            }
                            Ok((g.found.clone(), g.access.clone()))
                        }
                        _ => Err(fault),
                    }
                }
                Err(fault) => {
                    // Lasting, or the grace is off: nothing old is kept.
                    self.failing.remove(&key);
                    self.good.remove(&key);
                    Err(fault)
                }
            };
            match &held {
                Ok((found, access)) => {
                    rows.extend(
                        found
                            .iter()
                            .filter(|r| space_of(r).is_some_and(|s| spaces.contains(s)))
                            .cloned(),
                    );
                    let mut access = access.clone();
                    access.retain(|s| spaces.contains(s));
                    tenant.extend(access);
                }
                Err(e) => {
                    unread.push(unread_line(&target, spaces.len(), &e.text));
                    for space in &spaces {
                        unread_spaces.push((
                            space.clone(),
                            "the database this space is served from could not be read when the \
                             door was built"
                                .to_string(),
                        ));
                    }
                }
            }
        }
        // A database that could not be read is read again next time rather
        // than answered from here.
        self.targets.retain(|_, t| t.held.is_ok());

        // The single-database directory is ordered by space then id, and the
        // engine is built in that order; keep it, so a rebuild is deterministic
        // however many databases the rows came from.
        rows.sort();
        unread_spaces.sort();
        let engine = crate::engine_from_directory(
            f,
            &rows,
            &tenant,
            &crate::presence::current(),
            &crate::pause::current(),
            &crate::delegable::current(),
            &crate::access_template::current(),
        )?;
        Ok(Directory {
            engine,
            unread,
            unread_spaces,
            kept_stale,
            dialled,
            databases: live.len(),
        })
    }
}

fn space_of(row: &TextRow) -> Option<&str> {
    row.first().and_then(|c| c.as_deref())
}

fn unread_line(target: &DbAttachment, spaces: usize, e: &str) -> String {
    format!(
        "{}:{}/{} holds {spaces} space(s) whose members could not be read: {e}",
        target.host, target.port, target.db
    )
}
