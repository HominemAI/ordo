//! Open share links: sign-in-less, read-only, expiring, revocable
//! (`plan-sharing.md` §4, L1 to L3).
//!
//! A share link is a bearer credential. Whoever holds the secret reads, with
//! no account. It is built on the seat and the share grant that a guest has
//! (G1 to G5), so the door, the guards, the audit and the revocation are the
//! ones already argued. A link token is a window token whose principal
//! nobody owns.
//!
//! # What minting makes
//!
//! `std.space.shareLink` runs in the sharer's tenant, for a manager or an
//! owner, and makes four things, in the order of what is easiest to undo:
//!
//! 1. **A link seat.** A members row whose principal is `lnk_<random>`, with
//!    no role, named by the label. A principal that is not in the deployment's
//!    directory can never sign in.
//! 2. **The acl rows**, written as the sharer through the derived
//!    `grant<Entity>Access`, so they land in the sharer's own chain and
//!    succeed only where the sharer administers the row.
//! 3. **A share grant** (`shr_`) for the link seat, over the pins as
//!    validated now. The door asks the registry whether it is live on every
//!    call (G5's gate), so revoking is one row.
//! 4. **A `$sharelink` row** in the control database: the hash of the
//!    secret, the tenant, the grant, the seat, the ends and a count of opens.
//!
//! The secret is answered once. It is 256 random bits, only its hash is
//! stored, and it is in no chain, no stream and no listing.
//!
//! A failure after the seat exists takes back what this mint wrote.
//!
//! # What a link can never do
//!
//! The mint runs [`crate::share_ops::validate`], so a link lists only ops
//! that are deployed, are reads, are not `unguarded`, have a reachable guard,
//! are not platform ops and are within the sharer's own reach. A link
//! therefore cannot write, cannot call a platform op whatever
//! `ORDO_STEP_UP_EXEMPT_OPS` says, cannot act as the sharer and cannot share
//! onward (a link seat has no role, so it reaches no op by membership). It is
//! bound to one tenant. Files are the one addition: see below.
//!
//! # Lifetime, in three layers
//!
//! The platform sets no maximum (owner, 2026-09-29: "no max (not enforced by
//! us)"). What exists is layered:
//!
//! | Layer | Whose | What it does |
//! |---|---|---|
//! | Deployment default | deployment owner, `ORDO_SHARE_LINK_DEFAULT_SECS` | the lifetime of a link minted with no `$expires`, when its tenant has chosen nothing. 7 days when unset (`BlobPolicy::share_ttl_default`). Never a cap |
//! | Tenant choice | tenant owner, `std.space.setShareLinkLifetime`, only when the app developer exposes it with `ORDO_SHARE_LINK_TENANT_LIFETIME=on` | replaces the default for the tenant's links and is the longest a link of that tenant may be, because the tenant said so |
//! | The link | the sharer, `$expires` | its own lifetime, within what its tenant allows |
//!
//! The tenant's choice is stored on the tenant's `$admissionpolicy` row, the
//! way E4's admission policy stores its per-tenant values. It moves to the
//! general tenant settings store when that exists (checklist 24a).
//!
//! # Opening
//!
//! `POST /auth/link {link}` with no token ([`crate::auth::link`]). Unknown,
//! malformed, expired and revoked are one answer. Nothing here limits wrong
//! secrets: a link secret is 256 random bits, so guessing was never what a
//! limit stopped. Wrong passwords are limited, per link (below). In proxy mode `ORDO_SHARE_LINK_MISS_LIMIT` slows an address
//! that keeps missing, and never refuses it ([`crate::proxy`]). The answer is
//! a capability token for the link seat, its pins
//! decoded from the registry as consented, ending at the earlier of the
//! link's end and one session life.
//!
//! # A password (owner, 2026-10-22)
//!
//! A sharer may give a link a password, at the mint (`$password`) or later
//! with `std.space.setLinkPassword`, which also clears it. Whoever opens the
//! link then gives the password before any grant is issued.
//!
//! - Only an argon2id hash is stored ([`ordo_identity::Argon2idHasher`], the
//!   same as a person's password). It is in no answer, no listing and no
//!   stream. `std.space.links` says only `hasPassword`.
//! - Opening with no password is `link_password_required`. A wrong one is
//!   `link_password_wrong`. Both come only after the secret matched a live
//!   link, so they say nothing a holder of the secret did not know.
//! - Wrong tries are counted per link. The lock is **off by default**
//!   (owner, 2026-10-22): a wrong password is refused, one try at a time,
//!   and never locks. Where a layer sets `tries` to a number, the link locks
//!   for a time at that many wrong tries (`link_locked`, even for the right
//!   password), and the lock is on the tenant's stream (`link-locked`). The
//!   right password resets the count, and so does setting a new password.
//! - Wrong tries are also **slowed, always on** (owner, 2026-10-25): after 5
//!   in a row, each further try waits 1 s, 2 s, 4 s, ... up to 60 s
//!   (`429 too_many_attempts`, `Retry-After`), counted per link across
//!   nodes ([`crate::guess_limits`]). It never locks. The values are
//!   `slowAfter` and `slowCapSecs` among the limits below.
//! - A new password is at least 8 characters by default (owner, 2026-10-25).
//!   A link given a shorter one before keeps it.
//!
//! The wrong-try limit, the lock and the shortest password are layered the
//! way session settings are: the deployment's defaults and absolute limits,
//! the app developer's values and ranges, and a tenant's own, looser or
//! tighter, inside them ([`crate::link_password`]).
//!
//! # A limit on opens, and the end date (owner, 2026-10-22)
//!
//! - **Expiry** exists: `$expires` at the mint, else the layered default
//!   (`ORDO_SHARE_LINK_DEFAULT_SECS`, 7 days), and a tenant cap
//!   (`std.space.setShareLinkLifetime`). Past it a link is `expired`.
//! - **Maximum opens**: `$maxOpens` at the mint, and later
//!   `std.space.setLinkMaxOpens` (a number, or `none`). Once the link has been
//!   opened that many times it is **used up**. A used-up link is refused like
//!   an expired one, `403 link_refused`, so a holder learns nothing. The
//!   stream keeps the reason (`link-used-up`), and `std.space.links` shows
//!   `state: "used-up"`, `opensUsed` and `maxOpens`.
//! - **What counts as an open.** The first successful open by a device. A
//!   page that renews its token before it ends is not a new open, and neither
//!   is a reload on the same device. The client sends a random `device` id
//!   with `POST /auth/link`; the host keeps only its salted hash per link. A
//!   renewal by a counted device is allowed even after the limit is reached,
//!   so one viewer is not cut off mid-reading. A client that sends no `device`
//!   cannot be told from a new one, so each of its opens counts. Wrong
//!   passwords and refusals never count.
//! - **Atomic.** A new device is claimed with one guarded `UPDATE` on the
//!   link row (`uses < max_opens`), so two opens at once cannot both pass the
//!   last place.
//!
//! # Files
//!
//! `GET /blob/{ref}` checks the pseudo-op `std.$blob.get`, then the row guard
//! over the file's row (`PlatformBlobs::read`). A link that lists any
//! `std.<entity>.get` therefore also carries `std.$blob.get` in its grant, so
//! its holder reaches the file of a row the link's seat has an acl row on and
//! no other. Nothing else is added.

use std::sync::Arc;

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_identity::directory::random_principal_suffix;
use ordo_identity::grants::register_share;
use ordo_identity::principal::{GrantId, PrincipalId, SpaceId, Timestamp};
use ordo_identity::{AccessEngine, Argon2idHasher, PasswordHasher};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::auth::store_failure::IntoFault;
use crate::link_password::{self as pw, Deployment as PwDeployment, Effective, Field, Layer};
use crate::platform_ops::PLATFORM_VERSION;
use crate::share_accept::Window;
use crate::share_ops::{
    duration_secs, parse_rows, validate, ShareOps, ShareRefusal, ShareRow, Timing, GRANT_PREFIX,
};

pub const SHARE_LINK: &str = "std.space.shareLink";
pub const REVOKE_LINK: &str = "std.space.revokeLink";
pub const LINKS: &str = "std.space.links";
pub const SET_LINK_LIFETIME: &str = "std.space.setShareLinkLifetime";

pub const SET_LINK_PASSWORD: &str = "std.space.setLinkPassword";
/// A link's maximum number of opens: a number, or `none`.
pub const SET_LINK_MAX_OPENS: &str = "std.space.setLinkMaxOpens";
/// A tenant's own password limits, inside the app developer's range.
pub const SET_PASSWORD_LIMITS: &str = "std.space.setLinkPasswordLimits";
/// The app developer's password limits, in the deployment's own space.
pub const SET_APP_PASSWORD_LIMITS: &str = "std.deployment.setLinkPasswordLimits";

/// Every op of this module, for registration.
pub const OPS: [&str; 8] = [
    SHARE_LINK,
    REVOKE_LINK,
    LINKS,
    SET_LINK_LIFETIME,
    SET_LINK_PASSWORD,
    SET_LINK_MAX_OPENS,
    SET_PASSWORD_LIMITS,
    SET_APP_PASSWORD_LIMITS,
];

/// A link password is at most this many characters.
pub const PASSWORD_MAX: usize = crate::link_password::PASSWORD_MAX as usize;
/// Where the app developer's limits are kept in `$deploymentsetting`.
pub(crate) const APP_LIMITS_KEY: &str = "share_link_password_limits";

/// A link seat's principal starts with this. It is not a principal the
/// directory mints, so nobody can sign in as one.
pub const PRINCIPAL_PREFIX: &str = "lnk_";

/// A link's secret is this and 64 hex characters: 256 random bits.
pub const SECRET_PREFIX: &str = "lks_";
const SECRET_BODY_LEN: usize = 64;

/// The settings a deployment owner and an app developer give links.
pub const LINKS_ENV: &str = "ORDO_SHARE_LINKS";
pub const DEFAULT_SECS_ENV: &str = "ORDO_SHARE_LINK_DEFAULT_SECS";
pub const TENANT_LIFETIME_ENV: &str = "ORDO_SHARE_LINK_TENANT_LIFETIME";

/// A label is at most this many characters.
const MAX_LABEL: usize = 120;

/// The settings a deployment's links run under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkSettings {
    /// `ORDO_SHARE_LINKS`. Off refuses the mint and every open.
    pub enabled: bool,
    /// The deployment layer: the lifetime of a link minted with no
    /// `$expires` in a tenant that chose nothing. Not a cap.
    pub default_secs: u64,
    /// The app developer exposes the tenant layer.
    pub tenant_choice: bool,
    /// The deployment layer of the password limits: the defaults and the
    /// absolute limits ([`crate::link_password`]).
    pub password: PwDeployment,
}

impl Default for LinkSettings {
    /// On, seven days, the tenant layer not exposed.
    fn default() -> LinkSettings {
        LinkSettings {
            enabled: true,
            default_secs: ordo_blob::BlobPolicy::DEFAULT.share_ttl_default.as_secs(),
            tenant_choice: false,
            password: PwDeployment::default(),
        }
    }
}

fn switch(var: &'static str, default: bool) -> Result<bool, (&'static str, String)> {
    match std::env::var(var) {
        Err(_) => Ok(default),
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "" => Ok(default),
            "on" | "true" | "yes" | "1" => Ok(true),
            "off" | "false" | "no" | "0" => Ok(false),
            other => Err((var, format!("`{other}` is `on` or `off`"))),
        },
    }
}

impl LinkSettings {
    /// Read the variables. A value that is not understood refuses the
    /// boot rather than being read as the default.
    pub fn from_env() -> Result<LinkSettings, (&'static str, String)> {
        let base = LinkSettings::default();
        let default_secs = match std::env::var(DEFAULT_SECS_ENV) {
            Err(_) => base.default_secs,
            Ok(v) if v.trim().is_empty() => base.default_secs,
            Ok(v) => match v.trim().parse::<u64>() {
                Ok(n) if n > 0 => n,
                _ => {
                    return Err((
                        DEFAULT_SECS_ENV,
                        format!("`{v}` is not a positive number of seconds"),
                    ))
                }
            },
        };
        Ok(LinkSettings {
            enabled: switch(LINKS_ENV, base.enabled)?,
            default_secs,
            tenant_choice: switch(TENANT_LIFETIME_ENV, base.tenant_choice)?,
            password: PwDeployment::from_env()?,
        })
    }
}

/// The links table, in the control database.
pub const LINK_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$sharelink" (
  "hash"       text   NOT NULL,
  "id"         text   NOT NULL,
  "space"      text   NOT NULL,
  "grant_id"   text   NOT NULL,
  "seat"       text   NOT NULL,
  "label"      text   NOT NULL DEFAULT '',
  "ops"        text   NOT NULL DEFAULT '',
  "shared"     text   NOT NULL DEFAULT '',
  "issued_by"  text   NOT NULL DEFAULT '',
  "created_at" bigint NOT NULL,
  "expires_at" bigint NOT NULL,
  "revoked_at" bigint,
  "opens"      bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("hash"),
  UNIQUE ("id")
)"#;

/// The tenant's own choice, on its admission policy row. `NULL` inherits.
pub const POLICY_LINK_DDL: &str =
    r#"ALTER TABLE "$admissionpolicy" ADD COLUMN IF NOT EXISTS "share_link_secs" bigint"#;

/// A link's password: its argon2id hash (`NULL` is none), the wrong tries
/// since the last right one, and the end of a lock. Added, so a live table
/// from before keeps its links, all without a password.
pub const LINK_PASSWORD_DDL: &str = r#"ALTER TABLE "$sharelink"
  ADD COLUMN IF NOT EXISTS "password_hash" text,
  ADD COLUMN IF NOT EXISTS "password_misses" bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS "locked_until" bigint"#;

/// A link's limit on opens (`NULL` is none) and the opens used so far: the
/// devices that have opened it. Added, so a live table keeps its links, all
/// unlimited.
pub const LINK_MAX_OPENS_DDL: &str = r#"ALTER TABLE "$sharelink"
  ADD COLUMN IF NOT EXISTS "max_opens" bigint,
  ADD COLUMN IF NOT EXISTS "uses" bigint NOT NULL DEFAULT 0"#;

/// The devices that have opened a link: a salted hash, never the id itself.
pub const LINK_DEVICE_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$sharelinkdevice" (
  "link"     text   NOT NULL,
  "device"   text   NOT NULL,
  "first_at" bigint NOT NULL,
  PRIMARY KEY ("link","device")
)"#;

/// The tenant's own password limits. `NULL` inherits.
pub const POLICY_PASSWORD_DDL: &str = r#"ALTER TABLE "$admissionpolicy"
  ADD COLUMN IF NOT EXISTS "link_password_tries" bigint,
  ADD COLUMN IF NOT EXISTS "link_password_lock_secs" bigint,
  ADD COLUMN IF NOT EXISTS "link_password_min_length" bigint,
  ADD COLUMN IF NOT EXISTS "link_password_slow_after" bigint,
  ADD COLUMN IF NOT EXISTS "link_password_slow_cap_secs" bigint"#;

/// Create what links keep. The admission policy table is made first, because
/// the tenant's choice is a column of it. The wrong-password counts are
/// [`crate::guess_limits`]'s table.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    crate::admission::ensure_table(pool).await?;
    crate::guess_limits::ensure_counts(pool).await?;
    for ddl in [
        LINK_DDL,
        POLICY_LINK_DDL,
        LINK_PASSWORD_DDL,
        LINK_MAX_OPENS_DDL,
        LINK_DEVICE_DDL,
        POLICY_PASSWORD_DDL,
    ] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// The hash a secret is stored under.
pub fn hash_of(secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"ordo.sharelink.v1\0");
    h.update(secret.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn well_formed_secret(secret: &str) -> bool {
    secret
        .strip_prefix(SECRET_PREFIX)
        .is_some_and(|b| b.len() == SECRET_BODY_LEN && b.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn mint_secret() -> String {
    format!(
        "{SECRET_PREFIX}{}{}",
        random_principal_suffix(),
        random_principal_suffix()
    )
}

/// The specs of this module's ops.
pub fn specs() -> Vec<OpSpec> {
    let param = |name: &str, ty: ParamTy, required: bool| ParamSpec {
        name: name.to_string(),
        ty,
        required,
        bounds: None,
    };
    let text = |name: &str, max: u32, required: bool| {
        param(name, ParamTy::Text { max_len: Some(max) }, required)
    };
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, out: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        sync: if kind == OpKind::Read {
            Syncability::NotSyncable(NotSyncable::NoAnchor)
        } else {
            Syncability::NotSyncable(NotSyncable::NotARead)
        },
        kind,
        params,
        output_fields: out.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            SHARE_LINK,
            OpKind::Write,
            vec![
                text("ops", 4096, true),
                text("rows", 16384, true),
                param("expires", ParamTy::Duration, false),
                text("label", 256, false),
                text("password", PASSWORD_MAX as u32, false),
                text("maxOpens", 16, false),
            ],
            &[
                "link",
                "id",
                "seat",
                "expiresAt",
                "ops",
                "outcome",
                "hasPassword",
                "maxOpens",
            ],
        ),
        spec(
            REVOKE_LINK,
            OpKind::Write,
            vec![text("link", 64, true)],
            &["id", "seat", "grants", "aclRows", "outcome", "door"],
        ),
        spec(
            LINKS,
            OpKind::Read,
            Vec::new(),
            &[
                "id",
                "label",
                "ops",
                "rows",
                "opens",
                "createdAt",
                "expiresAt",
                "state",
                "issuedBy",
                "hasPassword",
                "lockedUntil",
                "opensUsed",
                "maxOpens",
            ],
        ),
        spec(
            SET_LINK_LIFETIME,
            OpKind::Write,
            vec![text("lifetime", 16, true)],
            &[
                "lifetimeSecs",
                "source",
                "deploymentDefaultSecs",
                "tenantChoice",
            ],
        ),
        spec(
            SET_LINK_PASSWORD,
            OpKind::Write,
            vec![
                text("link", 64, true),
                text("password", PASSWORD_MAX as u32, false),
            ],
            &["id", "hasPassword", "outcome"],
        ),
        spec(
            SET_LINK_MAX_OPENS,
            OpKind::Write,
            vec![text("link", 64, true), text("maxOpens", 16, true)],
            &["id", "maxOpens", "opensUsed", "state", "outcome"],
        ),
        spec(
            SET_PASSWORD_LIMITS,
            OpKind::Write,
            pw::FIELDS
                .iter()
                .map(|f| text(f.word(), 16, false))
                .collect(),
            &pw::answer_fields::<Field>(),
        ),
        spec(
            SET_APP_PASSWORD_LIMITS,
            OpKind::Write,
            pw::FIELDS
                .iter()
                .flat_map(|f| [text(f.word(), 16, false), text(f.range_word(), 32, false)])
                .collect(),
            &pw::answer_fields::<Field>(),
        ),
    ]
}

fn refused(why: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, why.into())
}

fn unavailable(e: impl IntoFault) -> ExecError {
    crate::auth::store_failure::exec_unavailable(e)
}

/// `$password`, untrimmed: absent or empty is none. Its length is checked
/// against the tenant's minimum here, and it is never echoed.
fn password_of(bound: &BoundParams, limits: &Effective) -> Result<Option<String>, ExecError> {
    let Some(p) = bound
        .0
        .get("password")
        .and_then(|v| v.as_str())
        .filter(|p| !p.is_empty())
    else {
        return Ok(None);
    };
    let n = p.chars().count();
    let min = limits.get(Field::MinLength) as usize;
    if n < min || n > PASSWORD_MAX {
        let whose = match limits.from_of(Field::MinLength) {
            "tenant" => format!("the tenant's minimum ({SET_PASSWORD_LIMITS} minLength)"),
            "app" => format!("the app developer's minimum ({SET_APP_PASSWORD_LIMITS} minLength)"),
            _ => format!("the deployment's minimum ({})", pw::MIN_LENGTH_ENV),
        };
        return Err(refused(format!(
            "link_bad_password: a link password here is {min} to {PASSWORD_MAX} characters. \
             {min} is {whose}"
        )));
    }
    Ok(Some(p.to_string()))
}

/// `$maxOpens`: `None` leaves it, `Some(None)` is no limit (`none`), and
/// `Some(Some(n))` is a limit of `n` opens, at least 1.
fn max_opens_of(bound: &BoundParams) -> Result<Option<Option<u64>>, ExecError> {
    let Some(word) = text(bound, "maxOpens") else {
        return Ok(None);
    };
    if word.eq_ignore_ascii_case("none") {
        return Ok(Some(None));
    }
    match word.parse::<u64>() {
        Ok(n) if (1..=MAX_OPENS_CEILING).contains(&n) => Ok(Some(Some(n))),
        _ => Err(refused(format!(
            "link_bad_max_opens: `maxOpens` is a whole number from 1 to {MAX_OPENS_CEILING}, \
             or `none`"
        ))),
    }
}

/// The most opens a link may be limited to.
pub const MAX_OPENS_CEILING: u64 = 1_000_000;

fn text<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Why a link was not opened. `Refused` is the one answer, with a fixed word
/// for the stream that the caller never sees. The password answers come only
/// after the secret matched a live link.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenFailure {
    Refused(&'static str),
    /// The link has a password and none was given.
    PasswordRequired,
    /// The password given is not the link's.
    PasswordWrong,
    /// Too many wrong passwords: the link opens again at `until`.
    Locked {
        until: u64,
    },
    /// Too many wrong passwords in a row: this try waits
    /// ([`crate::guess_limits`]). `setting` is the op that changes it.
    Throttled {
        retry_after: u64,
        setting: &'static str,
    },
    /// A store did not answer, with its typed class.
    Unavailable(ordo_identity::StoreFault),
}

/// What opening a link made.
#[derive(Debug, Clone)]
pub struct Opened {
    pub space: String,
    /// The link's id, which is its seat's principal.
    pub link: String,
    pub window: Window,
}

/// The state the link ops and the open route need.
pub struct ShareLinks {
    /// The share state: the members store, the grant registry, the door's
    /// engine and the acl runner, the stream and the clock.
    pub share: Arc<ShareOps>,
    pub settings: LinkSettings,
    /// How a link password is hashed and checked: argon2id.
    pub hasher: Arc<dyn PasswordHasher>,
    /// The deployment's own space, where the app developer's limits op
    /// answers. `None` answers anywhere: tests.
    pub home: Option<String>,
    /// The wrong-password counts, in the control database, so every node
    /// shares a link's count.
    pub counter: crate::guess_limits::Counter,
}

/// A link seat's lifetimes, from the layers.
struct Lifetime {
    /// What a link with no `$expires` gets.
    default_secs: u64,
    /// The longest a link may be, when the tenant chose one.
    cap: Option<u64>,
}

impl ShareLinks {
    pub fn new(share: Arc<ShareOps>, settings: LinkSettings) -> ShareLinks {
        let counter = crate::guess_limits::Counter::on_pool(share.members.control.clone());
        ShareLinks {
            share,
            settings,
            hasher: Arc::new(Argon2idHasher::new()),
            home: None,
            counter,
        }
    }

    /// Another hasher. Tests use `Argon2idHasher::weak_for_tests`.
    pub fn with_hasher(mut self, hasher: Arc<dyn PasswordHasher>) -> ShareLinks {
        self.hasher = hasher;
        self
    }

    pub fn with_home(mut self, home: impl Into<String>) -> ShareLinks {
        self.home = Some(home.into());
        self
    }

    /// Hash off the async threads: argon2id is slow on purpose.
    async fn hash_password(&self, password: String) -> Result<String, ExecError> {
        let hasher = self.hasher.clone();
        tokio::task::spawn_blocking(move || hasher.hash(&password))
            .await
            .map_err(|_| ExecError::internal("the password could not be hashed"))
    }

    /// Check off the async threads. argon2 compares in constant time.
    async fn verify_password(&self, password: String, hash: String) -> bool {
        let hasher = self.hasher.clone();
        tokio::task::spawn_blocking(move || hasher.verify(&password, &hash))
            .await
            .unwrap_or(false)
    }

    fn control(&self) -> &Pool {
        &self.share.members.control
    }

    /// The tenant's own choice, when there is one and the app developer
    /// exposes it. A stored choice is ignored while it is not exposed.
    async fn tenant_choice(&self, space: &str) -> Result<Option<u64>, ExecError> {
        if !self.settings.tenant_choice {
            return Ok(None);
        }
        let rows = self
            .control()
            .rows(&format!(
                r#"SELECT "share_link_secs" FROM "$admissionpolicy" WHERE "space" = {}"#,
                sql_lit(space)
            ))
            .await
            .map_err(unavailable)?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n > 0))
    }

    async fn lifetime_of(&self, space: &str) -> Result<Lifetime, ExecError> {
        let tenant = self.tenant_choice(space).await?;
        Ok(Lifetime {
            default_secs: tenant.unwrap_or(self.settings.default_secs),
            cap: tenant,
        })
    }

    /// Run one of this module's platform ops.
    pub async fn execute(
        &self,
        engine: &AccessEngine,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op {
            SHARE_LINK => self.mint(engine, space, bound, caller).await,
            REVOKE_LINK => self.revoke(space, bound, caller).await,
            LINKS => self.list(space).await,
            SET_LINK_LIFETIME => self.set_lifetime(space, bound, caller).await,
            SET_LINK_PASSWORD => self.set_password(space, bound, caller).await,
            SET_LINK_MAX_OPENS => self.set_max_opens(space, bound, caller).await,
            SET_PASSWORD_LIMITS => self.set_tenant_limits(space, bound, caller).await,
            SET_APP_PASSWORD_LIMITS => self.set_app_limits(space, bound, caller).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a share link op"
            ))),
        }
    }

    // ── L1: the mint ────────────────────────────────────────────────

    async fn mint(
        &self,
        engine: &AccessEngine,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if !self.settings.enabled {
            return Err(refused(format!(
                "share_links_off: this deployment has switched share links off. Set {LINKS_ENV}=on \
                 to turn them on"
            )));
        }
        let wiring = self.share.accept.as_ref().ok_or_else(|| {
            refused(
                "this deployment has no authentication configured (ORDO_AUTH_METHODS), so a \
                 share has nobody to be redeemed by",
            )
        })?;
        let ispace = SpaceId(space.to_string());
        let sharer = engine
            .principal_of_member(&ispace, caller)
            .cloned()
            .unwrap_or_else(|| PrincipalId(caller.to_string()));
        let ops: Vec<String> = text(bound, "ops")
            .ok_or_else(|| ExecError::internal("`$ops` did not reach the op"))?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let rows = parse_rows(
            text(bound, "rows")
                .ok_or_else(|| ExecError::internal("`$rows` did not reach the op"))?,
        )
        .map_err(|r| refused(r.to_string()))?;
        let label = text(bound, "label").unwrap_or("").to_string();
        if label.chars().count() > MAX_LABEL {
            return Err(refused(format!(
                "share_bad_label: a label is at most {MAX_LABEL} characters"
            )));
        }
        let password = password_of(
            bound,
            &self.password_limits(space).await.map_err(unavailable)?,
        )?;
        let max_opens = max_opens_of(bound)?.flatten();

        // The lifetime: the link's own, within what its tenant allows, or
        // the default of the layer that applies. The platform adds no cap.
        let layers = self.lifetime_of(space).await?;
        let lifetime = match text(bound, "expires") {
            Some(t) => duration_secs(t).ok_or_else(|| {
                refused(
                    ShareRefusal::BadExpiry("`$expires` is a duration like `7d`".into())
                        .to_string(),
                )
            })?,
            None => layers.default_secs,
        };
        let now = (self.share.now)();
        let valid = validate(
            engine,
            &ispace,
            &sharer,
            &ops,
            &rows,
            Timing {
                now,
                lifetime_secs: lifetime,
                max_secs: layers.cap,
            },
        )
        .map_err(|r| refused(r.to_string()))?;

        // A link that lists a `std.<entity>.get` carries `std.$blob.get`
        // too: `validate` adds it (`share_ops::carry_files`).

        // Hashed before anything is written, so a failure leaves nothing.
        let password_hash = match password {
            Some(p) => Some(self.hash_password(p).await?),
            None => None,
        };
        let link_id = format!("{PRINCIPAL_PREFIX}{}", random_principal_suffix());
        let secret = mint_secret();
        let display = if label.is_empty() {
            "share link".to_string()
        } else {
            label.clone()
        };

        // 1. The seat.
        let pool = self.share.members.pool_of(space).await?;
        let seat = format!("mem_{}", random_principal_suffix());
        pool.rows(&format!(
            r#"INSERT INTO "members"
               ("$space","id","rev","principal","displayName","kind","memberCount",
                "role","status","joined")
               VALUES ({s}, {m}, 1, {p}, {d}, 'principal', 1, NULL, 'active', CURRENT_DATE)"#,
            s = sql_lit(space),
            m = sql_lit(&seat),
            p = sql_lit(&link_id),
            d = sql_lit(&display),
        ))
        .await
        .map_err(unavailable)?;
        let undo_seat = |written: Vec<ShareRow>| {
            let pool = pool.clone();
            let seat = seat.clone();
            let space = space.to_string();
            let acl = wiring.acl.clone();
            let caller = caller.to_string();
            async move {
                for done in &written {
                    let _ = acl
                        .revoke(&space, &done.entity, &done.id, &seat, &caller)
                        .await;
                }
                let _ = pool
                    .rows(&format!(
                        r#"DELETE FROM "members" WHERE "$space" = {} AND "id" = {}
                           AND "role" IS NULL"#,
                        sql_lit(&space),
                        sql_lit(&seat)
                    ))
                    .await;
            }
        };

        // 2. The acl rows, as the sharer.
        let mut written: Vec<ShareRow> = Vec::new();
        for row in &valid.rows {
            match wiring
                .acl
                .grant(space, &row.entity, &row.id, &seat, &row.level, caller)
                .await
            {
                Ok(()) => written.push(row.clone()),
                Err(why) => {
                    undo_seat(written).await;
                    self.note_refusal(space, caller, &why, now);
                    return Err(refused(
                        "share_row_refused: you may not open one of those rows to a link",
                    ));
                }
            }
        }

        // 3. The grant, under the `shr_` prefix the door's gate asks about.
        let grant = GrantId(format!(
            "{GRANT_PREFIX}{}{}",
            random_principal_suffix(),
            random_principal_suffix()
        ));
        let new = match register_share(
            grant.clone(),
            ispace.clone(),
            PrincipalId(link_id.clone()),
            valid.pins.clone(),
            Timestamp(now),
            Timestamp(valid.expires_at),
        ) {
            Ok(n) => n,
            Err(e) => {
                undo_seat(written).await;
                return Err(refused(format!("guest_exposure_unguarded: {e}")));
            }
        };
        if let Err(e) = self.share.grants.record(&new).await {
            undo_seat(written).await;
            return Err(unavailable(e));
        }

        // 4. The link row.
        let shared = json!(valid
            .rows
            .iter()
            .map(|r| json!({"entity": r.entity, "id": r.id, "level": r.level}))
            .collect::<Vec<_>>())
        .to_string();
        let stored = self
            .control()
            .rows(&format!(
                r#"INSERT INTO "$sharelink"
                   ("hash","id","space","grant_id","seat","label","ops","shared","issued_by",
                    "created_at","expires_at","password_hash","max_opens")
                   VALUES ({h}, {i}, {s}, {g}, {seat}, {l}, {o}, {sh}, {by}, {now}, {end}, {ph},
                           {mo})"#,
                h = sql_lit(&hash_of(&secret)),
                i = sql_lit(&link_id),
                s = sql_lit(space),
                g = sql_lit(&grant.0),
                seat = sql_lit(&seat),
                l = sql_lit(&label),
                o = sql_lit(&valid.ops.join(",")),
                sh = sql_lit(&shared),
                by = sql_lit(&sharer.0),
                end = valid.expires_at,
                ph = password_hash.as_deref().map_or("NULL".to_string(), sql_lit),
                mo = max_opens.map_or("NULL".to_string(), |n| n.to_string()),
            ))
            .await;
        if let Err(e) = stored {
            let _ = self
                .share
                .grants
                .revoke(&ispace, &grant, Timestamp(now))
                .await;
            undo_seat(written).await;
            return Err(unavailable(e));
        }

        // The door, here and on every replica, so the seat is served.
        let door = self.share.members.rebuilt(space).await;
        self.record(
            "link-minted",
            space,
            caller,
            format!(
                "link={link_id} seat={seat} grant={} ops={} rows={} ends={} by={} password={} \
                 max_opens={} door={door}",
                grant.0,
                valid.ops.join(","),
                valid.rows.len(),
                valid.expires_at,
                sharer.0,
                if password_hash.is_some() { "yes" } else { "no" },
                max_opens.map_or("none".to_string(), |n| n.to_string()),
            ),
            false,
            now,
        );
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "link": secret,
                "id": link_id,
                "seat": seat,
                "expiresAt": valid.expires_at,
                "ops": valid.ops,
                "outcome": "minted",
                "hasPassword": password_hash.is_some(),
                "maxOpens": max_opens,
            })],
            next_cursor: None,
        })
    }

    // ── L3: revoke and list ─────────────────────────────────────────

    async fn revoke(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let id = text(bound, "link")
            .ok_or_else(|| ExecError::internal("`$link` did not reach the op"))?;
        let not_found = || ExecError::new(ErrorCode::NotFound, "this tenant has no such link");
        if !id.starts_with(PRINCIPAL_PREFIX) || !ordo_api::well_formed_space(id) {
            return Err(not_found());
        }
        let rows = self
            .control()
            .rows(&format!(
                r#"SELECT "seat" FROM "$sharelink" WHERE "id" = {} AND "space" = {}"#,
                sql_lit(id),
                sql_lit(space)
            ))
            .await
            .map_err(unavailable)?;
        let seat = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.clone())
            .ok_or_else(not_found)?;
        // The seat's grants are revoked first (the door refuses the next
        // call), then the acl rows are taken back and the seat is suspended.
        let closed = self.share.close(space, &seat, id, true, caller).await?;
        let now = (self.share.now)();
        self.control()
            .rows(&format!(
                r#"UPDATE "$sharelink" SET "revoked_at" = {now}
                   WHERE "id" = {} AND "space" = {} AND "revoked_at" IS NULL"#,
                sql_lit(id),
                sql_lit(space)
            ))
            .await
            .map_err(unavailable)?;
        self.record(
            "link-revoked",
            space,
            caller,
            format!(
                "link={id} seat={seat} grants={} acl_rows={} by={caller}",
                closed.grants, closed.acl_rows
            ),
            false,
            now,
        );
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "id": id,
                "seat": seat,
                "grants": closed.grants,
                "aclRows": closed.acl_rows,
                "outcome": "link revoked",
                "door": closed.door,
            })],
            next_cursor: None,
        })
    }

    /// The tenant's links, newest first. The secret is not stored and the
    /// hash is not shown.
    async fn list(&self, space: &str) -> Result<ExecResult, ExecError> {
        let now = (self.share.now)();
        let rows = self
            .control()
            .rows(&format!(
                r#"SELECT "id","label","ops","shared","opens","created_at","expires_at",
                          "revoked_at","issued_by",
                          ("password_hash" IS NOT NULL)::text,"locked_until",
                          "uses","max_opens"
                   FROM "$sharelink" WHERE "space" = {} ORDER BY "created_at" DESC, "id""#,
                sql_lit(space)
            ))
            .await
            .map_err(unavailable)?;
        let out = rows
            .iter()
            .map(|r| {
                let cell = |i: usize| r.get(i).and_then(|c| c.clone()).unwrap_or_default();
                let num = |i: usize| cell(i).parse::<u64>().unwrap_or(0);
                let max_opens = cell(12).parse::<u64>().ok();
                let state = if !cell(7).is_empty() {
                    "revoked"
                } else if num(6) <= now {
                    "expired"
                } else if max_opens.is_some_and(|m| num(11) >= m) {
                    "used-up"
                } else {
                    "live"
                };
                json!({
                    "id": cell(0),
                    "label": cell(1),
                    "ops": cell(2).split(',').filter(|o| !o.is_empty()).collect::<Vec<_>>(),
                    "rows": serde_json::from_str::<serde_json::Value>(&cell(3))
                        .unwrap_or(serde_json::Value::Null),
                    "opens": num(4),
                    "createdAt": num(5),
                    "expiresAt": num(6),
                    "state": state,
                    "issuedBy": cell(8),
                    "hasPassword": cell(9) == "true",
                    "lockedUntil": match num(10) {
                        n if n > now => json!(n),
                        _ => serde_json::Value::Null,
                    },
                    "opensUsed": num(11),
                    "maxOpens": max_opens,
                })
            })
            .collect();
        Ok(ExecResult {
            position: None,
            rows: out,
            next_cursor: None,
        })
    }

    // ── the tenant layer ────────────────────────────────────────────

    /// `std.space.setShareLinkLifetime`: the tenant owner's choice of how
    /// long its links may live. `default` takes the choice back.
    async fn set_lifetime(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if !self.settings.tenant_choice {
            return Err(refused(
                "share_link_lifetime_not_exposed: the app developer has not given tenants a \
                 choice of link lifetime. Set ORDO_SHARE_LINK_TENANT_LIFETIME=on to expose it",
            ));
        }
        let word = text(bound, "lifetime")
            .ok_or_else(|| ExecError::internal("`$lifetime` did not reach the op"))?;
        let chosen: Option<u64> = if word.eq_ignore_ascii_case("default") {
            None
        } else {
            match duration_secs(word) {
                Some(n) if n > 0 => Some(n),
                _ => {
                    return Err(refused(
                        "share_bad_expiry: a lifetime is a duration like `30d`, or `default`",
                    ))
                }
            }
        };
        let now = (self.share.now)();
        self.control()
            .rows(&format!(
                r#"INSERT INTO "$admissionpolicy" ("space","share_link_secs","updated_by","updated_at")
                   VALUES ({}, {}, {}, {now})
                   ON CONFLICT ("space") DO UPDATE SET
                     "share_link_secs" = EXCLUDED."share_link_secs",
                     "updated_by" = EXCLUDED."updated_by",
                     "updated_at" = EXCLUDED."updated_at""#,
                sql_lit(space),
                chosen.map_or("NULL".to_string(), |n| n.to_string()),
                sql_lit(caller),
            ))
            .await
            .map_err(unavailable)?;
        self.record(
            "link-lifetime-set",
            space,
            caller,
            format!(
                "lifetime={} by={caller}",
                chosen.map_or("default".to_string(), |n| format!("{n}s"))
            ),
            false,
            now,
        );
        let layers = self.lifetime_of(space).await?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "lifetimeSecs": layers.default_secs,
                "source": if layers.cap.is_some() { "tenant" } else { "deployment" },
                "deploymentDefaultSecs": self.settings.default_secs,
                "tenantChoice": true,
            })],
            next_cursor: None,
        })
    }

    // ── L2: opening ─────────────────────────────────────────────────

    /// Open a link. One answer for unknown, malformed, expired, revoked and
    /// a grant that is no longer live. A link with a password then wants it,
    /// before any grant is read or issued.
    pub async fn open(
        &self,
        secret: &str,
        password: Option<&str>,
        device: Option<&str>,
        now: u64,
    ) -> Result<Opened, OpenFailure> {
        if !self.settings.enabled {
            return Err(OpenFailure::Refused("links-off"));
        }
        let miss = OpenFailure::Refused;
        if !well_formed_secret(secret) {
            return Err(miss("link-malformed"));
        }
        let hash = hash_of(secret);
        let rows = self
            .control()
            .rows(&format!(
                r#"SELECT "id","space","expires_at","revoked_at","password_hash","locked_until"
                   FROM "$sharelink" WHERE "hash" = {}"#,
                sql_lit(&hash)
            ))
            .await
            .map_err(|e| OpenFailure::Unavailable(e.fault()))?;
        let Some(r) = rows.first() else {
            return Err(miss("link-unknown"));
        };
        let cell = |i: usize| r.get(i).and_then(|c| c.clone()).unwrap_or_default();
        let (id, space) = (cell(0), cell(1));
        if !cell(3).is_empty() {
            return Err(miss("link-revoked"));
        }
        if cell(2).parse::<u64>().unwrap_or(0) <= now {
            return Err(miss("link-expired"));
        }
        let stored = cell(4);
        if !stored.is_empty() {
            self.check_password(&hash, &id, &space, &stored, &cell(5), password, now)
                .await?;
        }
        // The consented pins and the grant's end, and a grant that is no
        // longer live refuses like any other.
        let window = match self.share.window(&space, &id).await {
            Ok(w) => w,
            Err(crate::share_accept::ShareFailure::Refused(_)) => {
                return Err(miss("link-grant-not-live"))
            }
            Err(crate::share_accept::ShareFailure::Unavailable(e)) => {
                return Err(OpenFailure::Unavailable(e))
            }
        };
        self.claim_open(&hash, &id, &space, device, now).await?;
        self.record("link-opened", &space, &id, format!("link={id}"), true, now);
        Ok(Opened {
            space,
            link: id,
            window,
        })
    }

    /// Count this open against the link's maximum, atomically. A device
    /// that has opened before is a renewal and is not counted, and is let
    /// through even at the limit. A new device (or one with no id) takes a
    /// place with one guarded `UPDATE`: when none is left it is refused, and
    /// a device row it wrote is taken back. Also bumps the activity count
    /// `opens`.
    async fn claim_open(
        &self,
        hash: &str,
        id: &str,
        space: &str,
        device: Option<&str>,
        now: u64,
    ) -> Result<(), OpenFailure> {
        let unavailable = |e: PoolError| OpenFailure::Unavailable(e.fault());
        let device_hash = device
            .filter(|d| !d.is_empty() && d.len() <= 128)
            .map(|d| hash_of(&format!("{id}\0{d}")));
        if let Some(dh) = &device_hash {
            let fresh = self
                .control()
                .rows(&format!(
                    r#"INSERT INTO "$sharelinkdevice" ("link","device","first_at")
                       VALUES ({}, {}, {now})
                       ON CONFLICT ("link","device") DO NOTHING RETURNING "link""#,
                    sql_lit(id),
                    sql_lit(dh),
                ))
                .await
                .map_err(unavailable)?;
            if fresh.is_empty() {
                // A renewal: the device is already counted.
                self.control()
                    .rows(&format!(
                        r#"UPDATE "$sharelink" SET "opens" = "opens" + 1 WHERE "hash" = {}"#,
                        sql_lit(hash)
                    ))
                    .await
                    .map_err(unavailable)?;
                return Ok(());
            }
        }
        let taken = self
            .control()
            .rows(&format!(
                r#"UPDATE "$sharelink" SET "uses" = "uses" + 1, "opens" = "opens" + 1
                   WHERE "hash" = {} AND ("max_opens" IS NULL OR "uses" < "max_opens")
                   RETURNING "uses""#,
                sql_lit(hash)
            ))
            .await
            .map_err(unavailable)?;
        if taken.is_empty() {
            if let Some(dh) = &device_hash {
                let _ = self
                    .control()
                    .rows(&format!(
                        r#"DELETE FROM "$sharelinkdevice" WHERE "link" = {} AND "device" = {}"#,
                        sql_lit(id),
                        sql_lit(dh)
                    ))
                    .await;
            }
            self.record(
                "link-used-up",
                space,
                id,
                format!("link={id} why=max-opens"),
                false,
                now,
            );
            return Err(OpenFailure::Refused("link-used-up"));
        }
        Ok(())
    }

    /// The password gate. Locked first, so the right password does not
    /// open a locked link. A wrong one is counted in one statement, so two
    /// at once cannot both slip under the limit.
    #[allow(clippy::too_many_arguments)]
    async fn check_password(
        &self,
        hash: &str,
        id: &str,
        space: &str,
        stored: &str,
        locked_until: &str,
        given: Option<&str>,
        now: u64,
    ) -> Result<(), OpenFailure> {
        let unavailable = |e: ordo_identity::StoreFault| OpenFailure::Unavailable(e);
        if let Ok(until) = locked_until.parse::<u64>() {
            if until > now {
                self.record(
                    "link-refused",
                    space,
                    id,
                    format!("link={id} why=locked"),
                    false,
                    now,
                );
                return Err(OpenFailure::Locked { until });
            }
        }
        let Some(given) = given.filter(|p| !p.is_empty()) else {
            return Err(OpenFailure::PasswordRequired);
        };
        let limits = self
            .password_limits(space)
            .await
            .map_err(|e| unavailable(e.fault()))?;
        // The slowdown (owner, 2026-10-25): counted before the check, so a
        // burst cannot slip under it; a try inside its wait is not checked.
        let attempt = self
            .counter
            .attempt(
                crate::guess_limits::KIND_LINK,
                id,
                limits.get(Field::SlowAfter),
                limits.get(Field::SlowCapSecs),
                now,
            )
            .await
            .map_err(|e| unavailable(e.fault()))?;
        if let crate::guess_limits::Attempt::Wait { retry_after } = attempt {
            // On the tenant's stream, as the link's seat: a denial, so a
            // burst is one collapsed entry carrying its count.
            if let Some(stream) = &self.share.stream {
                stream.observe(
                    ordo_secevent::Observation {
                        space: Some(ordo_commandlog::SpaceId(space.to_string())),
                        subject: ordo_secevent::Subject::member(id),
                        op: "auth.link".to_string(),
                        kind: ordo_secevent::EventKind::denied(
                            ordo_secevent::RecordedDecision::Throttled,
                        ),
                        at: ordo_commandlog::Timestamp(now),
                        correlation: ordo_secevent::correlation::mint(),
                    },
                    ordo_secevent::Relationship::Member,
                );
            }
            return Err(OpenFailure::Throttled {
                retry_after,
                setting: SET_PASSWORD_LIMITS,
            });
        }
        if self
            .verify_password(given.to_string(), stored.to_string())
            .await
        {
            self.counter
                .reset(crate::guess_limits::KIND_LINK, id)
                .await
                .map_err(|e| unavailable(e.fault()))?;
            self.control()
                .rows(&format!(
                    r#"UPDATE "$sharelink" SET "password_misses" = 0, "locked_until" = NULL
                       WHERE "hash" = {}
                         AND ("password_misses" > 0 OR "locked_until" IS NOT NULL)"#,
                    sql_lit(hash)
                ))
                .await
                .map_err(|e| unavailable(e.fault()))?;
            return Ok(());
        }
        if limits.get(Field::Tries) == pw::TRIES_OFF {
            // The lock is off: refused one try at a time, never locked.
            self.record(
                "link-refused",
                space,
                id,
                format!("link={id} why=password-wrong"),
                false,
                now,
            );
            return Err(OpenFailure::PasswordWrong);
        }
        let until = now.saturating_add(limits.get(Field::LockSecs));
        let rows = self
            .control()
            .rows(&format!(
                r#"UPDATE "$sharelink" SET
                     "password_misses" = CASE WHEN "password_misses" + 1 >= {t} THEN 0
                                              ELSE "password_misses" + 1 END,
                     "locked_until" = CASE WHEN "password_misses" + 1 >= {t} THEN {until}
                                           ELSE "locked_until" END
                   WHERE "hash" = {h}
                   RETURNING "locked_until""#,
                t = limits.get(Field::Tries),
                h = sql_lit(hash),
            ))
            .await
            .map_err(|e| unavailable(e.fault()))?;
        let locked_now = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse::<u64>().ok())
            == Some(until);
        if locked_now {
            self.record(
                "link-locked",
                space,
                id,
                format!(
                    "link={id} tries={} until={until} tries_from={} lock_from={}",
                    limits.get(Field::Tries),
                    limits.from_of(Field::Tries),
                    limits.from_of(Field::LockSecs)
                ),
                false,
                now,
            );
            return Err(OpenFailure::Locked { until });
        }
        self.record(
            "link-refused",
            space,
            id,
            format!("link={id} why=password-wrong"),
            false,
            now,
        );
        Err(OpenFailure::PasswordWrong)
    }

    // ── the password and its limits ─────────────────────────────────

    /// `std.space.setLinkPassword`: set a link's password, or clear it when
    /// `$password` is absent or empty. Either way the wrong-try count and any
    /// lock start again.
    async fn set_password(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let id = text(bound, "link")
            .ok_or_else(|| ExecError::internal("`$link` did not reach the op"))?;
        let not_found = || ExecError::new(ErrorCode::NotFound, "this tenant has no such link");
        if !id.starts_with(PRINCIPAL_PREFIX) || !ordo_api::well_formed_space(id) {
            return Err(not_found());
        }
        let hash = match password_of(
            bound,
            &self.password_limits(space).await.map_err(unavailable)?,
        )? {
            Some(p) => Some(self.hash_password(p).await?),
            None => None,
        };
        let rows = self
            .control()
            .rows(&format!(
                r#"UPDATE "$sharelink" SET "password_hash" = {}, "password_misses" = 0,
                     "locked_until" = NULL
                   WHERE "id" = {} AND "space" = {} RETURNING "id""#,
                hash.as_deref().map_or("NULL".to_string(), sql_lit),
                sql_lit(id),
                sql_lit(space)
            ))
            .await
            .map_err(unavailable)?;
        if rows.is_empty() {
            return Err(not_found());
        }
        // A new password starts the slowdown's count again too.
        self.counter
            .reset(crate::guess_limits::KIND_LINK, id)
            .await
            .map_err(unavailable)?;
        let now = (self.share.now)();
        let (act, outcome) = if hash.is_some() {
            ("link-password-set", "password set")
        } else {
            ("link-password-cleared", "password cleared")
        };
        self.record(
            act,
            space,
            caller,
            format!("link={id} by={caller}"),
            false,
            now,
        );
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "id": id,
                "hasPassword": hash.is_some(),
                "outcome": outcome,
            })],
            next_cursor: None,
        })
    }

    /// `std.space.setLinkMaxOpens`: change a link's maximum number of opens,
    /// or take it away with `none`. A limit below the opens already used
    /// makes the link used up at once.
    async fn set_max_opens(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let id = text(bound, "link")
            .ok_or_else(|| ExecError::internal("`$link` did not reach the op"))?;
        let not_found = || ExecError::new(ErrorCode::NotFound, "this tenant has no such link");
        if !id.starts_with(PRINCIPAL_PREFIX) || !ordo_api::well_formed_space(id) {
            return Err(not_found());
        }
        let max = max_opens_of(bound)?
            .ok_or_else(|| refused("link_bad_max_opens: `maxOpens` is needed"))?;
        let rows = self
            .control()
            .rows(&format!(
                r#"UPDATE "$sharelink" SET "max_opens" = {}
                   WHERE "id" = {} AND "space" = {} RETURNING "uses""#,
                max.map_or("NULL".to_string(), |n| n.to_string()),
                sql_lit(id),
                sql_lit(space)
            ))
            .await
            .map_err(unavailable)?;
        let Some(row) = rows.first() else {
            return Err(not_found());
        };
        let used = row
            .first()
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let now = (self.share.now)();
        self.record(
            "link-max-opens-set",
            space,
            caller,
            format!(
                "link={id} max_opens={} used={used} by={caller}",
                max.map_or("none".to_string(), |n| n.to_string())
            ),
            false,
            now,
        );
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "id": id,
                "maxOpens": max,
                "opensUsed": used,
                "state": if max.is_some_and(|m| used >= m) { "used-up" } else { "live" },
                "outcome": if max.is_some() { "limit set" } else { "limit removed" },
            })],
            next_cursor: None,
        })
    }

    /// The app developer's layer, from `$deploymentsetting`.
    async fn app_layer(&self) -> Result<Layer, ordo_identity::StoreFault> {
        let rows = self
            .control()
            .rows(&format!(
                r#"SELECT "value" FROM "$deploymentsetting" WHERE "key" = {}"#,
                sql_lit(APP_LIMITS_KEY)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the app developer's link password limits could not be read",
                )
            })?;
        Ok(rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| Layer::from_json(&v))
            .unwrap_or_default())
    }

    /// The tenant's own values, from its admission policy row.
    async fn tenant_layer(&self, space: &str) -> Result<Layer, ordo_identity::StoreFault> {
        let cols: Vec<String> = pw::FIELDS
            .iter()
            .map(|f| format!("\"{}\"", f.column()))
            .collect();
        let rows = self
            .control()
            .rows(&format!(
                r#"SELECT {} FROM "$admissionpolicy" WHERE "space" = {}"#,
                cols.join(","),
                sql_lit(space)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the tenant's link password limits could not be read",
                )
            })?;
        let mut l = Layer::default();
        for (i, f) in pw::FIELDS.iter().enumerate() {
            l.set_value(
                *f,
                rows.first()
                    .and_then(|r| r.get(i))
                    .and_then(|c| c.as_deref())
                    .and_then(|s| s.parse::<u64>().ok()),
            );
        }
        Ok(l)
    }

    /// The limits in force in `space`: the tenant's, else the app
    /// developer's, else the deployment's ([`pw::resolve`]).
    pub async fn password_limits(
        &self,
        space: &str,
    ) -> Result<Effective, ordo_identity::StoreFault> {
        let app = self.app_layer().await?;
        let tenant = self.tenant_layer(space).await?;
        Ok(pw::resolve(&self.settings.password, &app, &tenant))
    }

    fn limits_row(e: &Effective) -> ExecResult {
        ExecResult {
            position: None,
            rows: vec![e.row()],
            next_cursor: None,
        }
    }

    /// `std.space.setLinkPasswordLimits`: the tenant owner's own values,
    /// looser or tighter, inside the app developer's range, or the
    /// deployment's limits when there is none. `default` takes one back.
    async fn set_tenant_limits(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let app = self.app_layer().await.map_err(unavailable)?;
        let mut t = self.tenant_layer(space).await.map_err(unavailable)?;
        for f in pw::FIELDS {
            if let Some(change) = pw::value_change(bound, f)? {
                t.set_value(f, change);
            }
        }
        let d = &self.settings.password;
        pw::check_tenant(d, &app, &t)?;
        let now = (self.share.now)();
        let opt = |v: Option<u64>| v.map_or("NULL".to_string(), |n| n.to_string());
        let cols: Vec<String> = pw::FIELDS
            .iter()
            .map(|f| format!("\"{}\"", f.column()))
            .collect();
        let vals: Vec<String> = pw::FIELDS.iter().map(|f| opt(t.value(*f))).collect();
        let sets: Vec<String> = pw::FIELDS
            .iter()
            .map(|f| format!("\"{0}\" = EXCLUDED.\"{0}\"", f.column()))
            .collect();
        self.control()
            .rows(&format!(
                r#"INSERT INTO "$admissionpolicy" ("space",{},"updated_by","updated_at")
                   VALUES ({}, {}, {}, {now})
                   ON CONFLICT ("space") DO UPDATE SET {},
                     "updated_by" = EXCLUDED."updated_by",
                     "updated_at" = EXCLUDED."updated_at""#,
                cols.join(","),
                sql_lit(space),
                vals.join(", "),
                sql_lit(caller),
                sets.join(", "),
            ))
            .await
            .map_err(unavailable)?;
        let detail: Vec<String> = pw::FIELDS
            .iter()
            .map(|f| format!("{}={}", f.word(), opt(t.value(*f))))
            .collect();
        self.record(
            "link-password-limits-set",
            space,
            caller,
            format!("{} by={caller}", detail.join(" ")),
            false,
            now,
        );
        Ok(Self::limits_row(&pw::resolve(d, &app, &t)))
    }

    /// `std.deployment.setLinkPasswordLimits`: the app developer's values
    /// and optional ranges, inside the deployment's limits. Answers only in
    /// the deployment's own space.
    async fn set_app_limits(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if self.home.as_deref().is_some_and(|h| h != space) {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "the app developer's link password limits are the deployment's setting, and \
                 this op answers only in the deployment's own space",
            ));
        }
        let mut a = self.app_layer().await.map_err(unavailable)?;
        for f in pw::FIELDS {
            if let Some(change) = pw::value_change(bound, f)? {
                a.set_value(f, change);
            }
            if let Some(change) = pw::range_change(bound, f)? {
                a.set_range(f, change);
            }
        }
        let d = &self.settings.password;
        pw::check_app(d, &a)?;
        let now = (self.share.now)();
        let value = a.to_json();
        self.control()
            .rows(&format!(
                r#"INSERT INTO "$deploymentsetting" ("key","value","updated_by","updated_at")
                   VALUES ({}, {}, {}, {now})
                   ON CONFLICT ("key") DO UPDATE SET "value" = EXCLUDED."value",
                     "updated_by" = EXCLUDED."updated_by", "updated_at" = EXCLUDED."updated_at""#,
                sql_lit(APP_LIMITS_KEY),
                sql_lit(&value.to_string()),
                sql_lit(caller),
            ))
            .await
            .map_err(unavailable)?;
        if let Some(stream) = &self.share.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "link-password-limits-set",
                    detail: format!("layer=app value={value} by={caller}"),
                    by: Some(caller),
                    tenant: None,
                    deployment: true,
                    at: now,
                },
            );
        }
        Ok(Self::limits_row(&pw::resolve(d, &a, &Layer::default())))
    }

    // ── the record ──────────────────────────────────────────────────

    fn record(&self, act: &'static str, space: &str, by: &str, detail: String, dep: bool, at: u64) {
        if let Some(stream) = &self.share.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act,
                    detail,
                    by: Some(by),
                    tenant: Some(space),
                    deployment: dep,
                    at,
                },
            );
        }
    }

    fn note_refusal(&self, space: &str, by: &str, why: &str, at: u64) {
        self.record(
            "link-refused",
            space,
            by,
            format!("why={}", why.replace(' ', "_")),
            false,
            at,
        );
    }
}
