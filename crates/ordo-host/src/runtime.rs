//! R1 — the server runtime: the environment, the assembly, and the socket.
//!
//! `ordo-host` was a library that returned a `Router` and nothing in the
//! workspace bound a port to it. This module is the missing half, split in
//! three so that `ordo dev` and a production server are the *same*
//! assembly:
//!
//! ```ignore
//! let cfg = RuntimeConfig::from_env()?;   // the ONLY environment reader
//! let rt  = assemble(&cfg).await?;        // pool, custody, gate, door
//! serve(rt).await                         // the socket, and the shutdown
//! ```
//!
//! `assemble` works over the **struct**, never over the environment, which
//! is what lets a dev loop build a config it composed itself and get
//! exactly the deployment a server gets — gate included. `plan-dev-loop.md`
//! §7 rejects "serve ungated in dev" outright, and this split is what makes
//! that rejection structural rather than a matter of discipline.
//!
//! # Refusal postures
//!
//! Three, and which one a variable gets is a decision:
//!
//! - **Refuse to boot** — the deployment is unsafe or incoherent without
//!   it. Every secret is here.
//! - **Refuse the feature, boot, and say so** — the platform is coherent
//!   without it, and pretending otherwise turns a missing option into an
//!   outage. The checkpoint keys are here.
//! - **Default, and publish the default** — a tuning knob with one obvious
//!   right answer.
//!
//! What is never available: silently defaulting a secret, and silently
//! disabling a mandatory control.
//!
//! **Every environment refusal precedes every dial.** A deployment missing
//! its token key fails in milliseconds with one message rather than after a
//! pool timeout against a database it should never have touched — and a
//! misconfigured replica in a rolling deploy dies before it takes a
//! connection slot from a healthy one.
//!
//! # Logging
//!
//! `println!`/`eprintln!`. No framework, no structured logs, no tracing.
//! The platform has none anywhere (`howto-operating.md`), and adding one
//! inside the entrypoint would make the binary's observability better than
//! the library's — which is backwards, and would choose a framework on
//! behalf of every crate in a plan about deployment.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use ordo_api::{AccessChecker, AccessDecision, ApiState, PageKey};
use ordo_conformance::Fixture;
use ordo_crypto::{KeyProvider, SpaceId};
use ordo_exec::{Pool, PoolConfig};

use crate::deploy::{self, Deployment, LoadRefusal, Swapped};
use crate::metrics::Metrics;
use crate::secdrain::SecurityDrain;
use crate::serve::PlatformState;

// ── configuration ────────────────────────────────────────────────────────

/// Which custody the operator selected.
///
/// **An explicit selector, never inferred** from which other variables
/// happen to be set: a deployment that meant `sealed-master` and mistyped
/// the file path must get a refusal, never a silent demotion to something
/// weaker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustodyChoice {
    /// `ORDO_MASTER_KEY` — a **plaintext master key in the process
    /// environment**. Kept and first-class for a laptop and a small
    /// self-host, and printed as such so the posture is never invisible.
    Local,
    /// `ORDO_MASTER_KEY_FILE` + `ORDO_MASTER_KEY_PASSPHRASE_FILE`. The file
    /// may travel with the deployment; the passphrase belongs in the
    /// orchestrator's secret store. Neither alone opens anything.
    SealedMaster {
        path: std::path::PathBuf,
        passphrase: std::path::PathBuf,
    },
    /// The repository's constant. Selectable only with
    /// `ORDO_DEV_UNSAFE_TEST_KEYS`.
    Test,
}

impl CustodyChoice {
    fn label(&self) -> &'static str {
        match self {
            CustodyChoice::Local => "local (env master key)",
            CustodyChoice::SealedMaster { .. } => "sealed-master (file)",
            CustodyChoice::Test => "test (UNSAFE — repository constant)",
        }
    }

    /// The master key source behind this choice, for the two arms that have
    /// one.
    fn source(&self) -> Option<ordo_crypto::master::MasterKeySource> {
        match self {
            CustodyChoice::Local => Some(ordo_crypto::master::MasterKeySource::Env),
            CustodyChoice::SealedMaster { path, passphrase } => {
                Some(ordo_crypto::master::MasterKeySource::SealedFile {
                    path: path.clone(),
                    passphrase: passphrase.clone(),
                })
            }
            CustodyChoice::Test => None,
        }
    }
}

/// Which store a space's bytes are in.
///
/// The bytes tier's switch (`plan-tenant-storage.md` §2 B-C1, §11 U5):
/// explicit, no default that changes behaviour, and an unknown value
/// refuses at boot rather than serving every tenant out of the
/// deployment's own bucket while looking healthy doing it.
///
/// The database tier once had a twin, `ORDO_PG_ROUTING`, and it is gone
/// (ruled 2026-10-06): every tenant has a database of its own, so there is
/// nothing left to choose there. A bucket of a tenant's own stays optional,
/// which is why this switch remains.
///
/// B-C1 also gives the argument for why the mode cannot be inferred from
/// the table having rows, and it is sharper here than on the database
/// side: `$spacebucket` rows can already be written by a deployment that
/// later turns routing on, because the op that writes them is gated on
/// registration rather than on a resolver. Inferring would start a
/// deployment reading attachments nobody configured it to honour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlobRouting {
    /// Every space's bytes are in the store `ORDO_BLOB_STORE` named. The
    /// default, and what every deployment before U5 was.
    #[default]
    Single,
    /// A space with a `$spacebucket` row resolves to its own bucket; every
    /// other space resolves to the deployment's store. Requires a
    /// credential provider, and refuses at boot without one.
    Attached,
}

/// Every §1.1 value, already validated. Holds no environment access of its
/// own: [`RuntimeConfig::from_env`] is the only reader in the binary.
pub struct RuntimeConfig {
    pub pg: PoolConfig,
    /// Whether a space's bytes are reached through one store or through
    /// its own attachment (18e U5). See [`BlobRouting`].
    pub blob_routing: BlobRouting,
    pub addr: SocketAddr,
    pub shutdown_grace: Duration,
    pub space: SpaceId,
    pub custody: CustodyChoice,
    /// The 32 bytes behind the token key. Held rather than the `TokenKey`
    /// itself because a program swap rebuilds the door and `TokenKey` is
    /// deliberately not `Clone`.
    pub token_secret: [u8; 32],
    pub page_key: PageKey,
    /// `None` disables the checkpoint and attestation features and says so.
    pub checkpoint: Option<ordo_commandlog::sign::Ed25519Signer>,
    /// Whether `ORDO_SECURITY_CHECKPOINT_KEY` is set and parses. The boot
    /// replaces it with whether a key was found at all, the set one or the
    /// one made at first boot (`security_key`); this is for the census.
    pub security_checkpoint: bool,
    /// Read, recorded, and not acted on — see [`RuntimeConfig::from_env`].
    pub apex_domain: Option<String>,
    /// The CORS allowlist (`ORDO_CORS_ORIGINS`, 14g). `None` is same-origin
    /// only, which is the door's behaviour before the setting existed.
    pub cors: Option<ordo_api::cors::CorsPolicy>,
    /// Where a space's blob bytes go, and how the log should name it.
    /// `None` is a deployment that has configured no store; whether that is
    /// legitimate is a question about the *program* — see [`blob_needed`].
    pub blob: Option<BlobBinding>,
    /// Where `std.space.export` writes bundles: `ORDO_EXIT_BUNDLE_DIR`.
    /// `None` leaves both exit ops unregistered and the census says so.
    pub exit_bundles: Option<std::path::PathBuf>,
    /// Where `std.space.import` READS staged bundles:
    /// `ORDO_IMPORT_BUNDLE_DIR`. `None` leaves the import op unregistered.
    ///
    /// A second directory rather than a second use of the one above, argued
    /// at [`crate::exit_ops::ExitOps::imports`]: exports land under a
    /// per-space tree, so an import that named a path inside it would be a
    /// caller naming another space's directory.
    pub import_bundles: Option<std::path::PathBuf>,
    /// This process's identity in `$usage_replica` (`ORDO_REPLICA_ID`,
    /// default eight random hex characters). A replica that restarts is a
    /// new replica — its rows still sum, and the old one's stay `'open'`
    /// as the record of what it lost.
    pub replica: String,
    /// `usage.flush`'s own cadence: `ORDO_USAGE_FLUSH_SECS`, default 60.
    /// A shorter flush costs one write per replica per interval and loses
    /// less on a crash; `howto-operating.md` states the trade.
    pub usage_flush: Duration,
    /// `usage.rollup`'s own cadence: `ORDO_USAGE_ROLLUP_SECS`, default
    /// 3600 (one hour — the billing grain, and it keeps `$usage` at 24
    /// rows per space per day). A shorter rollup makes quotas and
    /// dashboards fresher at the cost of more `$usage` rows;
    /// `howto-operating.md` states the trade.
    pub usage_rollup: Duration,
    /// How a sync feed paces its reads: `ORDO_SYNC_PAGE_ROWS` (default 50,
    /// 1 to 1,000) and `ORDO_SYNC_PAGE_WAIT_MS` (default 50, 0 to 10,000).
    /// Read once, here, and nowhere else; `ApiState.sync` carries them.
    pub sync: ordo_api::SyncPacing,
    /// The authentication surface: `None` when `ORDO_AUTH_METHODS` is
    /// unset, which is a deployment that verifies tokens somebody else
    /// minted and says so in the census.
    pub auth: Option<AuthConfig>,
    /// The tenant credential provider: `None` when `ORDO_TENANT_CREDENTIALS`
    /// is unset, which is legitimate: the label `$deployment` is answered
    /// with the deployment's own password either way, and only a cluster
    /// dialled under a label of its own needs one. See
    /// [`credentials_from_env`] and [`tenant_credentials`].
    pub credentials: Option<Arc<ordo_exec::creds::Dir>>,
    /// Which platform ops require a fresh passkey assertion: the app
    /// developer's setting, `ORDO_STEP_UP_EXEMPT_OPS`, and the strict
    /// `ORDO_PRESENCE_ON_READS`. The default is every op that changes
    /// something. See [`crate::presence`].
    pub presence: crate::presence::PresencePolicy,
    /// Which tenant roles may pause and resume their own tenant (27ar): the
    /// app developer's setting, `ORDO_TENANT_PAUSE_ROLES` and
    /// `ORDO_TENANT_RESUME_ROLES`. The default is none, which leaves pausing
    /// to the deployment owner. See [`crate::pause::PausePolicy`].
    pub pausing: crate::pause::PausePolicy,
    /// Which delete ops a delegated token may call: the app developer's
    /// setting, `ORDO_DELEGABLE_OPS`. The default is none, and platform
    /// writes cannot be opened at all. See [`crate::delegable`].
    pub delegable: crate::delegable::DelegablePolicy,
    /// The app developer's default per op: which roles may call it in a
    /// tenant that has not changed it (`ORDO_ACCESS_TEMPLATE`). The default
    /// names no op. See [`crate::access_template`].
    pub access_template: crate::access_template::AccessTemplate,
    /// The principals made owners of the deployment's own space at boot:
    /// `ORDO_DEPLOYMENT_OWNERS`. Empty is the default. See
    /// [`crate::provision::ensure_deployment_owners`].
    pub deployment_owners: Vec<String>,
    /// What the environment says about self-serve provisioning and its two
    /// limits. It seeds the stored setting on a boot that finds none. See
    /// [`crate::self_serve`].
    pub provisioning: crate::self_serve::SelfServeConfig,
    /// What credential recovery does and how long it takes: the deployment
    /// owner's settings, all `ORDO_RECOVERY_*`, each with a default and each
    /// refused rather than clamped. See [`crate::recovery::RecoveryPolicy`].
    pub recovery: crate::recovery::RecoveryPolicy,
    /// The second step: `ORDO_AUTH_SECOND_FACTOR` and its companions, each
    /// with a default and each refused rather than clamped. See
    /// [`crate::second_factors::SecondFactorConfig`].
    pub second_factor: crate::second_factors::SecondFactorConfig,
    /// The mail channel: `ORDO_MAIL_KIND` and its companions. `None`, the
    /// default, sends no mail. See [`crate::mail::MailChannel::from_vars`].
    pub mail: Option<crate::mail::MailChannel>,
}

/// What `ORDO_AUTH_METHODS` and its companions decided.
///
/// One variable names the methods, in the style `ORDO_KEY_PROVIDER` and
/// `ORDO_BLOB_STORE` already use: a word the operator writes, and a refusal
/// that names the words that exist when they write another one.
pub struct AuthConfig {
    /// Configured method names. **Only `password` is wireable today**, see
    /// [`RuntimeConfig::from_env`] for why a name this binary cannot serve
    /// is a refused boot rather than a quietly missing login button.
    pub methods: Vec<String>,
    /// `ORDO_SESSION_SECS`, default 3600. How long one token is good for.
    pub session_secs: u64,
    /// `ORDO_SESSION_MAX_SECS`, default 43200 (twelve hours). How long a
    /// session may be continued by `/auth/refresh` before a ceremony is
    /// required again.
    pub session_max_secs: u64,
    /// `ORDO_REMEMBER` and `ORDO_REMEMBER_SECS`: staying signed in across a
    /// reload, on by default for 30 idle days ([`crate::renewal`]).
    pub remember: crate::renewal::RememberConfig,
    /// The deployment's layer of the session policy: the four values above
    /// as defaults, and the `*_LIMIT` settings ([`crate::session_policy`]).
    pub session_layers: crate::session_policy::Deployment,
    /// Whether an authenticator nobody invited becomes an account.
    /// `ORDO_AUTH_PROVISIONING` = `invite-only` (default) | `auto-onboard`.
    pub auto_onboard: bool,
    /// The relying party, present exactly when `passkey` is among the
    /// methods. Built by [`relying_party_from_env`], which refuses a boot
    /// that names `passkey` without one.
    pub relying_party: Option<ordo_identity::RelyingParty>,
    /// The OAuth2 connections among the methods, each checked, with its
    /// client secret read from its file (`plan-auth-federated.md` §5.3).
    pub oauth2: Vec<ordo_identity::oauth2::OAuth2Connection>,
    /// The OIDC connections among the methods (`plan-auth-federated.md`
    /// §4): Google, Microsoft, and any issuer with discovery.
    pub oidc: Vec<ordo_identity::oidc::OidcConnection>,
    /// The deny list an outside provider's endpoints are checked against:
    /// the categories, plus `ORDO_EGRESS_DENY_CIDRS`.
    pub egress_deny: ordo_egress::DenyList,
    /// The anchors an outside provider's certificate is checked against: the
    /// public root program, plus the PEM file `ORDO_AUTH_PROVIDER_CA_FILE`
    /// names, for a provider behind a private CA. Unset, the public program
    /// alone.
    pub egress_roots: ordo_egress::TlsRoots,
    /// `ORDO_AUTH_HUMAN_CHECK`: the Turnstile check at `begin`. `None` is off,
    /// the default (`plan-auth-federated.md` §7.4 E15).
    pub human_check: Option<crate::auth::human::HumanCheck>,
    /// The emailed-code numbers, present exactly when `email` is a method
    /// (`plan-auth-federated.md` §7.4). It needs `ORDO_MAIL_KIND`.
    pub email: Option<crate::email_code::EmailCodePolicy>,
    /// The emailed-code numbers for confirming a person's own address
    /// ([`crate::contact_email`]): present whenever the deployment sends
    /// mail, whether or not `email` is a sign-in method.
    pub contact_codes: Option<crate::email_code::EmailCodePolicy>,
    /// `ORDO_EMAIL_REQUIRED`: which ops need a verified address. Off by
    /// default.
    pub email_required: crate::contact_email::Requirement,
    /// `ORDO_EMAIL_CHANGE_TOUCH`: a passkey touch to change or remove an
    /// email. On by default.
    pub email_change_touch: bool,
    /// `ORDO_AUTH_RETURN_URLS`, read by origin: where an outside provider
    /// and a subdomain hand-off may send the browser back to. `None` when
    /// unset, which leaves the hand-off refusing every return.
    pub returns: Option<ordo_identity::origin::OriginList>,
    /// `ORDO_AUTH_HANDOFF_SECS`, default 60: how long a hand-off code lives.
    pub handoff_secs: u64,
    /// `ORDO_AUTH_LINKING` (on by default) and `ORDO_AUTH_LINKING_SECS`
    /// (300): linking by claim (`plan-auth-federated.md` step 7).
    pub linking: crate::auth::claim::Linking,
    /// `ORDO_AUTH_GUEST` (off by default) and `ORDO_AUTH_GUEST_TTL_SECS`:
    /// guest sign-in (`plan-auth-federated.md` §7A). The first seeds the
    /// stored setting.
    pub guest: crate::auth::guest::GuestConfig,
    /// `ORDO_AUTH_CONSOLE_SESSION_SECS` (900): how long a console key
    /// session lasts (`plan-console.md` 8.2, G14).
    pub console_session_secs: u64,
    /// `ORDO_AUTH_PASSWORD_PENDING_MAX` (10000): how many password
    /// challenges may wait unanswered at once. A `begin` past it is refused
    /// naming the setting.
    pub password_pending_max: u64,
    /// `ORDO_AUTH_TENANT_PROVIDERS=on`: tenants may add their own sign-in
    /// providers (`plan-tenant-providers.md`). Holds `ORDO_PUBLIC_URL` when
    /// on; `None`, the default, is off.
    pub tenant_providers: Option<String>,
    /// `ORDO_AUTH_OIDC_*`: the deployment's OIDC layer, read once for every
    /// OIDC connection, the deployment's and each tenant's own.
    pub oidc_defaults: crate::oidc::OidcDefaults,
    /// `ORDO_AUTH_METHODS_MAX_AGE`: how long `GET /auth/methods` may be kept.
    pub methods_max_age: u64,
    /// `ORDO_AUTH_TENANT_LOCK_WAIT_MS`: how long a guarded change to a
    /// tenant's owners waits for the tenant's lock.
    pub tenant_lock_wait_ms: u64,
}

/// `ORDO_WILDCARD_ORIGINS`: `on` lets `https://*.example.com` stand in the
/// three origin lists. Off by default (owner ruling 2026-10-09).
pub fn wildcards_from_env() -> Result<bool, BootRefusal> {
    ordo_identity::origin::wildcards_setting(
        std::env::var(ordo_identity::origin::WILDCARD_SETTING)
            .ok()
            .as_deref(),
    )
    .map_err(|detail| BootRefusal::Invalid {
        var: ordo_identity::origin::WILDCARD_SETTING,
        detail,
    })
}

/// `ORDO_AUTH_HANDOFF_SECS`: 1 to 300, unset for the default.
fn handoff_secs_from(raw: Option<&str>) -> Result<u64, BootRefusal> {
    let default = crate::auth::handoff::DEFAULT_SECS;
    let Some(raw) = raw.filter(|r| !r.trim().is_empty()) else {
        return Ok(default);
    };
    match raw.trim().parse::<u64>() {
        Ok(n) if (1..=crate::auth::handoff::MAX_SECS).contains(&n) => Ok(n),
        _ => Err(BootRefusal::Invalid {
            var: "ORDO_AUTH_HANDOFF_SECS",
            detail: format!(
                "`{raw}` is not 1 to {} seconds; unset it for the default, {default}",
                crate::auth::handoff::MAX_SECS
            ),
        }),
    }
}

/// One bounded number setting: unset is `default`; anything outside
/// `min..=max` refuses the boot, naming the setting, the bounds and the
/// default.
fn bounded_setting(
    env: &dyn Fn(&str) -> Option<String>,
    var: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, BootRefusal> {
    let Some(raw) = env(var).filter(|r| !r.trim().is_empty()) else {
        return Ok(default);
    };
    match raw.trim().parse::<u64>() {
        Ok(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(BootRefusal::Invalid {
            var,
            detail: format!(
                "`{}` is not a number from {min} to {max}; unset it for the default, {default}",
                raw.trim()
            ),
        }),
    }
}

/// How many connections the door's store pool may hold
/// ([`store_pool_config`]).
pub const STORE_POOL_SIZE_ENV: &str = "ORDO_PG_STORE_POOL_SIZE";
/// The default for [`STORE_POOL_SIZE_ENV`]: 4.
///
/// About a dozen sign-in stores share this pool, each serialised through its
/// own job loop, so the most it can ever use at once is one per store. One
/// connection meant one slow commit turned every other store away (board
/// task `store-pool-held-blocks-signin`: one `INSERT` waited 2017 ms on its
/// WAL fsync and the ceremony store was refused `server_busy`). Four lets
/// three other stores carry on while one commit stalls. Connections dial on
/// demand and close after `ORDO_PG_IDLE_SECS`, so an idle deployment still
/// holds one, and a busy one at most 3 more than it did.
pub const STORE_POOL_SIZE_DEFAULT: u64 = 4;
/// The most [`STORE_POOL_SIZE_ENV`] accepts: more than one per store buys
/// nothing, since each store runs one statement at a time.
pub const STORE_POOL_SIZE_MAX: u64 = 32;
/// How long a store waits for a free connection in that pool, in
/// milliseconds ([`store_pool_config`]).
pub const STORE_ACQUIRE_TIMEOUT_ENV: &str = "ORDO_PG_STORE_ACQUIRE_TIMEOUT_MS";
/// The default for [`STORE_ACQUIRE_TIMEOUT_ENV`]: 10 seconds.
///
/// A store waits behind statements, not behind requests. A disk stall
/// slows every commit at once, so with all connections stuck the wait has
/// to outlast the stall. The stall seen was 2 s on a CI runner; cloud
/// volumes stall for a few seconds too. 10 s rides out one 5 times that
/// long, and stays well inside the 30 s `ORDO_PG_STATEMENT_TIMEOUT_MS`
/// and the time a person waits on a sign-in page. The request pool keeps
/// its own 2 s (`ORDO_PG_ACQUIRE_TIMEOUT_MS`): there a long wait is a
/// queue of requests, which should be refused early.
pub const STORE_ACQUIRE_TIMEOUT_DEFAULT_MS: u64 = 10_000;
/// The longest [`STORE_ACQUIRE_TIMEOUT_ENV`] accepts: a minute.
pub const STORE_ACQUIRE_TIMEOUT_MAX_MS: u64 = 60_000;

/// The door's store pool: `pg` resized by [`STORE_POOL_SIZE_ENV`] and
/// [`STORE_ACQUIRE_TIMEOUT_ENV`]. A value that is not a number in range
/// refuses naming the setting, the range and the default. Public so a test
/// builds the pool exactly as a server does.
pub fn store_pool_config(
    env: &dyn Fn(&str) -> Option<String>,
    pg: &PoolConfig,
) -> Result<PoolConfig, BootRefusal> {
    let size = bounded_setting(
        env,
        STORE_POOL_SIZE_ENV,
        STORE_POOL_SIZE_DEFAULT,
        1,
        STORE_POOL_SIZE_MAX,
    )?;
    let wait = bounded_setting(
        env,
        STORE_ACQUIRE_TIMEOUT_ENV,
        STORE_ACQUIRE_TIMEOUT_DEFAULT_MS,
        1,
        STORE_ACQUIRE_TIMEOUT_MAX_MS,
    )?;
    Ok(PoolConfig {
        size: size as usize,
        acquire_timeout: Duration::from_millis(wait),
        ..pg.clone()
    })
}

/// The sync feed's pacing: `ORDO_SYNC_PAGE_ROWS` and
/// `ORDO_SYNC_PAGE_WAIT_MS`. Unset is the default; a value outside its
/// bounds, or not a number, refuses the boot naming the setting.
fn sync_pacing_from(
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<ordo_api::SyncPacing, BootRefusal> {
    let (rows_min, rows_max) = ordo_api::SYNC_PAGE_ROWS_BOUNDS;
    let (wait_min, wait_max) = ordo_api::SYNC_PAGE_WAIT_MS_BOUNDS;
    let page_rows = bounded_setting(
        env,
        "ORDO_SYNC_PAGE_ROWS",
        ordo_api::DEFAULT_SYNC_PAGE_ROWS as u64,
        rows_min as u64,
        rows_max as u64,
    )?;
    let page_wait_ms = bounded_setting(
        env,
        "ORDO_SYNC_PAGE_WAIT_MS",
        ordo_api::DEFAULT_SYNC_PAGE_WAIT_MS,
        wait_min,
        wait_max,
    )?;
    Ok(ordo_api::SyncPacing {
        page_rows: page_rows as usize,
        page_wait: Duration::from_millis(page_wait_ms),
    })
}

/// The fixed limits that are settings: how long an outside provider's
/// hand-off lives, how long one provider request may take, and the mail
/// subject and text caps. Read at every boot and installed before any
/// request is served. (Turnstile's timeout and the recovery settings are read
/// where those are built.)
fn install_limits(env: &dyn Fn(&str) -> Option<String>) -> Result<(), BootRefusal> {
    let handoff = bounded_setting(
        env,
        "ORDO_AUTH_PROVIDER_HANDOFF_SECS",
        ordo_identity::HANDOFF_TTL_SECS,
        10,
        600,
    )?;
    let request = bounded_setting(
        env,
        "ORDO_AUTH_PROVIDER_REQUEST_SECS",
        crate::federation::FETCH_TIMEOUT.as_secs(),
        1,
        60,
    )?;
    let subject = bounded_setting(
        env,
        "ORDO_MAIL_SUBJECT_MAX_BYTES",
        ordo_egress::smtp::SUBJECT_CAP as u64,
        20,
        998,
    )?;
    let text = bounded_setting(
        env,
        "ORDO_MAIL_TEXT_MAX_BYTES",
        ordo_egress::smtp::TEXT_CAP as u64,
        1024,
        1024 * 1024,
    )?;
    ordo_identity::set_handoff_ttl_secs(handoff);
    crate::federation::set_fetch_timeout_secs(request);
    ordo_egress::smtp::set_caps(subject as usize, text as usize);
    Ok(())
}

/// A configured blob store, and the label the startup log prints.
///
/// **This is the last place a bare store is named, and it is a boot-time
/// value rather than a held one.** `ORDO_BLOB_STORE` describes one store,
/// so parsing it produces one store; what the serving paths hold is the
/// resolver built from it, and nothing downstream of [`serve`] takes a
/// `BlobStore` any more. When `AttachedStores` arrives
/// (`plan-tenant-storage.md` §11 U5) this binding becomes the deployment's
/// **default**, the store a space with no attachment row resolves to,
/// which is the same value doing the same job, one seam further down.
pub struct BlobBinding {
    pub store: Arc<dyn ordo_blob::BlobStore>,
    pub label: String,
    /// The deployment's **own** bucket, as the identity
    /// `std.space.attachBucket` refuses (18p §10 B-X3.1). `None` for a
    /// store that is not S3: an `fs:` test double has no endpoint and no
    /// bucket an attachment could name, and §2 B-C4 records that an
    /// attachment can never be an `fs:` target.
    ///
    /// Captured here, from the `S3Config` this function already built,
    /// rather than parsed back out of `label`: a label is for a human to
    /// read and a refusal that depended on its format would break the
    /// first time somebody improved the wording.
    pub own: Option<crate::bucket_ops::OwnStore>,
}

/// Why the binary refused to start.
#[derive(Debug)]
#[non_exhaustive]
pub enum BootRefusal {
    /// A required variable is unset. Carries the name and the one-line
    /// reason, because a refusal that names only the variable makes the
    /// operator go and read the source for the *why*.
    Missing {
        var: &'static str,
        why: String,
    },
    Invalid {
        var: &'static str,
        detail: String,
    },
    /// The pool would not dial. Carries the remedy for the one failure the
    /// server's own text does not name (see [`assemble`]).
    Database(String),
    Custody(ordo_crypto::KeyError),
    /// The platform's own home could not be brought into existence. A
    /// refusal rather than a warning: a deployment with no system space has
    /// nowhere to keep what it knows about itself, and serving anyway would
    /// make that absence a discovery for whoever writes the first
    /// configuration row.
    SystemSpace(crate::provision::ProvisionRefusal),
    Load(LoadRefusal),
    Engine(String),
    /// §0.3's second `TestProvider` check. Not reachable while
    /// `verify_candidate` asserts the same thing — which is the point of
    /// two independent mechanisms.
    CustodyMismatch,
    Serve(String),
}

impl std::fmt::Display for BootRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootRefusal::Missing { var, why } => write!(f, "{var} is not set — {why}"),
            BootRefusal::Invalid { var, detail } => write!(f, "{var}: {detail}"),
            BootRefusal::Database(m) => write!(f, "database: {m}"),
            BootRefusal::Custody(e) => write!(f, "custody: {e}"),
            BootRefusal::SystemSpace(e) => write!(
                f,
                "the system space `{}` could not be created: {e}",
                crate::provision::SYSTEM_SPACE
            ),
            BootRefusal::Load(e) => write!(f, "{e}"),
            BootRefusal::Engine(m) => write!(f, "access engine: {m}"),
            BootRefusal::CustodyMismatch => f.write_str(
                "the loaded program does not seal through this deployment's KeyProvider",
            ),
            BootRefusal::Serve(m) => write!(f, "serve: {m}"),
        }
    }
}

impl std::error::Error for BootRefusal {}

/// Read `ORDO_AUTH_METHODS` and its companions into an [`AuthConfig`].
///
/// # Why an unserviceable method refuses the boot
///
/// Of six `AuthMethodKind` variants, three have an implementation and only
/// **one** can be wired here. Passkey needs the browser ceremony and the
/// relying-party id (`ORDO_APEX_DOMAIN`), which is 5h's remaining deferral;
/// OIDC has `StubOidcProvider`, which models the authorization-code shape
/// and performs no network I/O, so wiring it would be a login button that
/// admits whoever posts the right JSON. SAML, OAuth2 and EmailOtp are enum
/// variants with no implementation at all.
///
/// An operator who writes `ORDO_AUTH_METHODS=google` has said what they
/// want. Starting anyway with that method silently absent would be a
/// deployment that looks configured and refuses every Google login, which
/// is discovered by a user rather than by the person who set the variable.
/// So it is a refusal, and the refusal names what exists.
///
/// # The directory gate is gone
///
/// The credential store, the session stores, and now the directory —
/// which authenticator belongs to which principal — are all durable.
/// `ordo_identity::directory::Directory` is built here over
/// `ordo_host::principal_directory::PgDirectoryStore` unconditionally,
/// the same way it is already built over `PgCredentialStore` below, so
/// `ORDO_AUTH_UNSAFE_MEMORY_DIRECTORY` no longer names anything unsafe to
/// gate. See `docs/roadmap-checklist.md` 20b for what closed and what the
/// durability tests in `ordo-host/tests/auth_durability.rs` prove.
fn auth_from_env(methods: &str) -> Result<AuthConfig, BootRefusal> {
    let methods: Vec<String> = methods
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    let env = |k: &str| std::env::var(k).ok();
    let wildcards = wildcards_from_env()?;
    // Where a sign-in may send the browser back to: an outside provider's
    // return leg and a subdomain hand-off (`plan-subdomain-signin.md`).
    // Optional here; required below by an outside provider.
    let returns = match env("ORDO_AUTH_RETURN_URLS") {
        Some(text) if !text.trim().is_empty() => Some(
            crate::federation::parse_return_urls(&text, wildcards).map_err(|detail| {
                BootRefusal::Invalid {
                    var: "ORDO_AUTH_RETURN_URLS",
                    detail,
                }
            })?,
        ),
        _ => None,
    };
    let handoff_secs = handoff_secs_from(env("ORDO_AUTH_HANDOFF_SECS").as_deref())?;
    let linking = crate::auth::claim::from_vars(&env)
        .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
    let guest = crate::auth::guest::GuestConfig::from_vars(&env)
        .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
    let console_session_secs =
        crate::console_keys::session_secs_from(&env).map_err(|detail| BootRefusal::Invalid {
            var: crate::console_keys::SESSION_SECS_ENV,
            detail,
        })?;
    let mut oauth2 = Vec::new();
    let mut oidc = Vec::new();
    for m in &methods {
        if m == "password" || m == "passkey" || m == crate::email_code::METHOD {
            continue;
        }
        if !crate::federation::names_a_connection(m, &env) {
            return Err(BootRefusal::Invalid {
                var: "ORDO_AUTH_METHODS",
                detail: format!(
                    "`{m}` is not a method this binary can serve. `password` and `passkey` \
                     and `email` are built in, and an OAuth2 connection is a name with its own \
                     settings ({p}PRESET=discord or github, or {p}KIND=oauth2), and an OIDC \
                     connection likewise ({p}PRESET=google or microsoft, or {p}KIND=oidc). \
                     SAML is not built yet",
                    p = crate::federation::prefix(m),
                ),
            });
        }
        let public_url = required(
            "ORDO_PUBLIC_URL",
            "an outside provider returns the browser to <ORDO_PUBLIC_URL>/auth/callback/<name>",
        )?;
        if !public_url.starts_with("https://") {
            return Err(BootRefusal::Invalid {
                var: "ORDO_PUBLIC_URL",
                detail: "the address providers return the browser to must be https://".into(),
            });
        }
        let Some(return_urls) = returns.clone() else {
            return Err(BootRefusal::Missing {
                var: "ORDO_AUTH_RETURN_URLS",
                why: "an outside provider's sign-in ends by sending the browser back to the \
                      app, and only to an address listed here"
                    .into(),
            });
        };
        let read_file = |path: &str| std::fs::read_to_string(path).map_err(|e| e.to_string());
        if crate::oidc::is_oidc(m, &env) {
            oidc.push(
                crate::oidc::connection_from_vars(m, &env, &read_file, &public_url, &return_urls)
                    .map_err(|detail| BootRefusal::Invalid {
                    var: "ORDO_AUTH_METHODS",
                    detail,
                })?,
            );
            continue;
        }
        let conn =
            crate::federation::connection_from_vars(m, &env, &read_file, &public_url, &return_urls)
                .map_err(|detail| BootRefusal::Invalid {
                    var: "ORDO_AUTH_METHODS",
                    detail,
                })?;
        oauth2.push(conn);
    }
    // The deployment's deny list and `ORDO_EGRESS_PRIVATE_HOSTS` are parsed
    // here once, at every boot, mail on or off. Sign-in (OIDC, OAuth2) and
    // Turnstile take clones of this list; mail parses the same two settings
    // through the same function.
    let egress_deny = ordo_egress::DenyList::from_vars(&env)
        .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
    let egress_roots = provider_roots(&env, &|path: &str| std::fs::read(path))?;
    let human_check = crate::auth::human::from_vars(
        &env,
        &|path: &str| std::fs::read_to_string(path).map_err(|e| e.to_string()),
        egress_deny.clone(),
    )
    .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
    // An emailed code is sent through Ordo's mail setting (owner,
    // 2026-10-08), so `email` without one would be a sign-in that mails
    // nobody. Refused by name.
    let email = if methods.iter().any(|m| m == crate::email_code::METHOD) {
        required(
            crate::mail::KIND_ENV,
            "the `email` method sends its codes through the deployment's mail setting \
             (smtp, resend or sendgrid)",
        )?;
        Some(
            crate::email_code::EmailCodePolicy::from_vars(&env)
                .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?,
        )
    } else {
        None
    };
    // A person's own address is confirmed with an emailed code through the
    // same mail setting, whatever the ways in (owner ruling 2026-10-22).
    let contact_codes = match (&email, env(crate::mail::KIND_ENV)) {
        (Some(p), _) => Some(p.clone()),
        (None, Some(_)) => Some(
            crate::email_code::EmailCodePolicy::from_vars(&env)
                .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?,
        ),
        (None, None) => None,
    };
    let email_required = crate::contact_email::Requirement::from_vars(&env).map_err(|detail| {
        BootRefusal::Invalid {
            var: crate::contact_email::REQUIRED_ENV,
            detail,
        }
    })?;
    let email_change_touch =
        crate::contact_email::touch_from_vars(&env).map_err(|detail| BootRefusal::Invalid {
            var: crate::contact_email::TOUCH_ENV,
            detail,
        })?;
    if !email_required.is_off() && contact_codes.is_none() {
        return Err(BootRefusal::Invalid {
            var: crate::contact_email::REQUIRED_ENV,
            detail: format!(
                "nobody could confirm an address, because this deployment sends no mail; set \
                 {} (smtp, resend or sendgrid), or {}=off",
                crate::mail::KIND_ENV,
                crate::contact_email::REQUIRED_ENV
            ),
        });
    }
    let relying_party = if methods.iter().any(|m| m == "passkey") {
        Some(relying_party_from_env(wildcards)?)
    } else {
        None
    };
    let session_secs: u64 = parsed(
        "ORDO_SESSION_SECS",
        ordo_identity::session::DEFAULT_SESSION_SECS,
    )?;
    let session_max_secs: u64 = parsed(
        "ORDO_SESSION_MAX_SECS",
        ordo_identity::session::DEFAULT_SESSION_MAX_SECS,
    )?;
    if session_secs == 0 {
        return Err(BootRefusal::Invalid {
            var: "ORDO_SESSION_SECS",
            detail: format!(
                "a token that expires at the instant it is minted authenticates nobody; \
                 unset it for the default, {}",
                ordo_identity::session::DEFAULT_SESSION_SECS
            ),
        });
    }
    if session_max_secs < session_secs {
        return Err(BootRefusal::Invalid {
            var: "ORDO_SESSION_MAX_SECS",
            detail: format!(
                "a ceiling of {session_max_secs}s below the token life of {session_secs}s \
                 would clip every token at the ceiling and make the shorter value the real \
                 session length"
            ),
        });
    }
    let remember = crate::renewal::from_vars(&env)
        .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
    let session_layers = crate::session_policy::Deployment::from_vars(
        &env,
        session_secs,
        session_max_secs,
        remember,
    )
    .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
    let auto_onboard = match std::env::var("ORDO_AUTH_PROVISIONING").as_deref() {
        Ok("auto-onboard") => true,
        Ok("invite-only") | Err(_) => false,
        Ok(other) => {
            return Err(BootRefusal::Invalid {
                var: "ORDO_AUTH_PROVISIONING",
                detail: format!("`{other}` is not a policy (invite-only | auto-onboard)"),
            })
        }
    };
    // Tenant-owned providers (`plan-tenant-providers.md`): off by default.
    // On, they need the same two addresses a deployment connection does.
    let tenant_providers =
        if crate::tenant_providers::enabled_from_vars(&env).map_err(|detail| {
            BootRefusal::Invalid {
                var: crate::tenant_providers::SETTING,
                detail,
            }
        })? {
            let public_url = required(
                "ORDO_PUBLIC_URL",
                "a tenant's provider returns the browser to \
             <ORDO_PUBLIC_URL>/auth/callback/t/<tenant>/<name>",
            )?;
            if !public_url.starts_with("https://") {
                return Err(BootRefusal::Invalid {
                    var: "ORDO_PUBLIC_URL",
                    detail: "the address providers return the browser to must be https://".into(),
                });
            }
            if returns.is_none() {
                return Err(BootRefusal::Missing {
                    var: "ORDO_AUTH_RETURN_URLS",
                    why: "a tenant's provider ends its sign-in by sending the browser back to the \
                      app, and only to an address listed here"
                        .into(),
                });
            }
            Some(public_url)
        } else {
            None
        };
    let oidc_defaults = crate::oidc::defaults_from_vars(&env)
        .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
    let tenant_lock_wait_ms = bounded_setting(
        &env,
        crate::tenant_providers::LOCK_WAIT_ENV,
        crate::tenant_providers::DEFAULT_LOCK_WAIT_MS,
        crate::tenant_providers::MIN_LOCK_WAIT_MS,
        crate::tenant_providers::MAX_LOCK_WAIT_MS,
    )?;
    let methods_max_age = bounded_setting(
        &env,
        crate::auth::methods::MAX_AGE_ENV,
        crate::auth::methods::DEFAULT_MAX_AGE,
        0,
        crate::auth::methods::MAX_MAX_AGE,
    )?;
    let password_pending_max = bounded_setting(
        &env,
        ordo_identity::provider::PASSWORD_PENDING_MAX_ENV,
        ordo_identity::provider::PASSWORD_PENDING_MAX as u64,
        10,
        1_000_000,
    )?;
    Ok(AuthConfig {
        password_pending_max,
        tenant_providers,
        oidc_defaults,
        methods_max_age,
        tenant_lock_wait_ms,
        returns,
        handoff_secs,
        linking,
        guest,
        console_session_secs,
        methods,
        session_secs,
        session_max_secs,
        remember,
        session_layers,
        auto_onboard,
        relying_party,
        oauth2,
        oidc,
        egress_deny,
        egress_roots,
        human_check,
        email,
        contact_codes,
        email_required,
        email_change_touch,
    })
}

/// `ORDO_AUTH_PROVIDER_CA_FILE`: a PEM file of extra trust anchors for the
/// outside sign-in providers (OAuth2 and OIDC, the deployment's and each
/// tenant's). Unset, the public root program alone. Set, the public program
/// plus every certificate in the file. A file that cannot be read or holds no
/// usable certificate refuses the boot by name. Certificate validation stays
/// on either way.
pub fn provider_roots(
    env: &dyn Fn(&str) -> Option<String>,
    read: &dyn Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<ordo_egress::TlsRoots, BootRefusal> {
    const VAR: &str = "ORDO_AUTH_PROVIDER_CA_FILE";
    let Some(path) = env(VAR)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return Ok(ordo_egress::TlsRoots::webpki());
    };
    let pem = read(&path).map_err(|e| BootRefusal::Invalid {
        var: VAR,
        detail: format!(
            "could not be read ({}); unset it to trust the public root program alone",
            e.kind()
        ),
    })?;
    ordo_egress::TlsRoots::webpki_and_pem(&pem).map_err(|detail| BootRefusal::Invalid {
        var: VAR,
        detail: format!("{detail}; it names a PEM file of CA certificates"),
    })
}

/// Build the relying party from the environment, when `passkey` is a method.
///
/// | Variable | Required | Meaning |
/// |---|---|---|
/// | `ORDO_APEX_DOMAIN` | yes | the RP id |
/// | `ORDO_AUTH_ORIGINS` | yes | origins under the RP id that may run a ceremony |
/// | `ORDO_AUTH_RELATED_ORIGINS` | no | origins on **other** domains (Related Origin Requests) |
/// | `ORDO_AUTH_RP_NAME` | no | the name an authenticator shows; default the RP id |
/// | `ORDO_AUTH_USER_VERIFICATION` | no | `required` (default) or `preferred` |
///
/// The origins are not derived from `ORDO_CORS_ORIGINS`. They answer a
/// different question: which page may run a WebAuthn ceremony, and not which
/// page may call the door. The owner ruled (2026-09-29) that passkeys may be
/// used on more than one address, so this is its own setting, and an address
/// on another domain is an explicit option, not a widening of the first.
fn relying_party_from_env(wildcards: bool) -> Result<ordo_identity::RelyingParty, BootRefusal> {
    use ordo_identity::UserVerification;
    let apex = required(
        "ORDO_APEX_DOMAIN",
        "passkey needs a relying-party id; changing it later strands every enrolled passkey",
    )?;
    let list = |var: &'static str| -> Vec<String> {
        std::env::var(var)
            .unwrap_or_default()
            .split(',')
            .map(|o| o.trim().to_string())
            .filter(|o| !o.is_empty())
            .collect()
    };
    let raw = required(
        "ORDO_AUTH_ORIGINS",
        "passkey needs the origins that may run a ceremony, for example https://app.example.com",
    )?;
    let origins: Vec<String> = raw
        .split(',')
        .map(|o| o.trim().to_string())
        .filter(|o| !o.is_empty())
        .collect();
    let refs: Vec<&str> = origins.iter().map(String::as_str).collect();
    let invalid = |var: &'static str| {
        move |e: ordo_identity::RelyingPartyError| BootRefusal::Invalid {
            var,
            detail: e.to_string(),
        }
    };
    let mut rp = ordo_identity::RelyingParty::with_wildcards(apex.trim(), &refs, wildcards)
        .map_err(|e| {
            // The apex and the origins are one construction; name the variable
            // the text is about.
            let var = match e {
                ordo_identity::RelyingPartyError::InvalidRpId { .. } => "ORDO_APEX_DOMAIN",
                _ => "ORDO_AUTH_ORIGINS",
            };
            let hint = match e {
                ordo_identity::RelyingPartyError::OriginNotUnderRpId { .. } => {
                    "; an address on another domain is `ORDO_AUTH_RELATED_ORIGINS`, and it \
                 needs `/.well-known/webauthn` published on the relying party's domain"
                }
                _ => "",
            };
            BootRefusal::Invalid {
                var,
                detail: format!("{e}{hint}"),
            }
        })?;
    let related = list("ORDO_AUTH_RELATED_ORIGINS");
    if !related.is_empty() {
        let refs: Vec<&str> = related.iter().map(String::as_str).collect();
        rp = rp
            .with_related_origins(&refs)
            .map_err(invalid("ORDO_AUTH_RELATED_ORIGINS"))?;
    }
    if let Ok(name) = std::env::var("ORDO_AUTH_RP_NAME") {
        if !name.trim().is_empty() {
            rp = rp.with_name(name.trim());
        }
    }
    match std::env::var("ORDO_AUTH_USER_VERIFICATION").as_deref() {
        Ok("required") | Err(_) => {}
        Ok("preferred") => rp = rp.with_user_verification(UserVerification::Preferred),
        Ok(other) => {
            return Err(BootRefusal::Invalid {
                var: "ORDO_AUTH_USER_VERIFICATION",
                detail: format!("`{other}` is not a setting (required | preferred)"),
            })
        }
    }
    Ok(rp)
}

fn required(var: &'static str, why: &str) -> Result<String, BootRefusal> {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(BootRefusal::Missing {
            var,
            why: why.to_string(),
        }),
    }
}

/// One refinement size limit as the boot reads it (`refinement-default` ask
/// 6). Unset is the default. A value that is not a number, or is 0, refuses
/// the boot, and the refusal names the variable and its default, as the
/// owner asked: a limit of 0 would refuse every refinement, and refinement
/// has no off switch (ruled 2026-10-05).
fn refine_limit(
    var: &'static str,
    raw: Option<&str>,
    default: usize,
) -> Result<usize, BootRefusal> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let limit: usize = raw.trim().parse().map_err(|_| BootRefusal::Invalid {
        var,
        detail: format!("`{raw}` is not a number; unset it for the default, {default}"),
    })?;
    if limit == 0 {
        return Err(BootRefusal::Invalid {
            var,
            detail: format!(
                "a limit of 0 refuses every refinement, and refinement has no off switch; \
                 unset it for the default, {default}"
            ),
        });
    }
    Ok(limit)
}

/// One of the `json` document limits (28b): a positive number, or unset
/// for the default.
fn json_limit(var: &'static str, raw: Option<&str>, default: usize) -> Result<usize, BootRefusal> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let limit: usize = raw.trim().parse().map_err(|_| BootRefusal::Invalid {
        var,
        detail: format!("`{raw}` is not a number; unset it for the default, {default}"),
    })?;
    if limit == 0 {
        return Err(BootRefusal::Invalid {
            var,
            detail: format!(
                "a limit of 0 refuses every json document; unset it for the default, {default}"
            ),
        });
    }
    Ok(limit)
}

fn parsed<T: std::str::FromStr + std::fmt::Display>(
    var: &'static str,
    default: T,
) -> Result<T, BootRefusal> {
    match std::env::var(var) {
        Err(_) => Ok(default),
        Ok(v) => v.trim().parse().map_err(|_| BootRefusal::Invalid {
            var,
            detail: format!("`{v}` is not a number; unset it for the default, {default}"),
        }),
    }
}

impl RuntimeConfig {
    /// `ORDO_CORS_ORIGINS`: a list of exact origins, or `None` when it is
    /// unset. A malformed entry and `*` refuse the boot, so a mistyped origin
    /// is not silently dropped and found by a browser page as a discarded
    /// answer (14g). Public so `ordo dev` reads it the way a server does.
    pub fn cors_from_env() -> Result<Option<ordo_api::cors::CorsPolicy>, BootRefusal> {
        match std::env::var("ORDO_CORS_ORIGINS") {
            Err(_) => Ok(None),
            Ok(list) => {
                let wildcards = wildcards_from_env()?;
                ordo_api::cors::CorsPolicy::parse_with(&list, wildcards).map_err(|detail| {
                    BootRefusal::Invalid {
                        var: "ORDO_CORS_ORIGINS",
                        detail,
                    }
                })
            }
        }
    }

    /// `ORDO_AUTH_METHODS` and its companions (`ORDO_APEX_DOMAIN`,
    /// `ORDO_AUTH_ORIGINS`, `ORDO_AUTH_PROVISIONING`, the session lengths):
    /// `None` when no method is named. Public so `ordo dev` reads them the
    /// way a server does.
    pub fn auth_from_env() -> Result<Option<AuthConfig>, BootRefusal> {
        match std::env::var("ORDO_AUTH_METHODS") {
            Ok(v) if !v.trim().is_empty() => Ok(Some(auth_from_env(&v)?)),
            _ => Ok(None),
        }
    }

    /// Read and validate the environment. **No I/O**, so every refusal
    /// here happens before the first dial.
    pub fn from_env() -> Result<RuntimeConfig, BootRefusal> {
        // ── the database ──
        //
        // Host, user and database refuse rather than default, because
        // `PoolConfig::from_env`'s defaults are a laptop's: a Unix socket
        // directory under `/tmp`, a throwaway cluster's non-default port,
        // and a test database's name. Inheriting any of them in a server is
        // how a deployment ends up pointed at nothing.
        let host = required(
            "ORDO_PG_HOST",
            "the pool would otherwise default to a socket directory on a laptop",
        )?;
        let user = required("ORDO_PG_USER", "a server has no default database user")?;
        let db = required(
            "ORDO_PG_DB",
            "`ordo_conformance` is a test database's name, not a default",
        )?;
        let port: u16 = parsed("ORDO_PG_PORT", 5432)?;
        // The password, or the file that holds it (`ordo_env::secret`).
        // Read here so an unreadable file refuses before the dial;
        // `PoolConfig::from_env` reads it again for the pool.
        ordo_env::secret("ORDO_PG_PASSWORD").map_err(|e| BootRefusal::Invalid {
            var: "ORDO_PG_PASSWORD_FILE",
            detail: e.to_string(),
        })?;
        let size: usize = parsed("ORDO_PG_POOL_SIZE", PoolConfig::from_env().size)?;
        if size == 0 {
            return Err(BootRefusal::Invalid {
                var: "ORDO_PG_POOL_SIZE",
                detail: format!(
                    "a pool of zero connections serves nothing; unset it for the default, {}",
                    PoolConfig::from_env().size
                ),
            });
        }
        let acquire_ms: u64 = parsed("ORDO_PG_ACQUIRE_TIMEOUT_MS", 2000)?;
        // How the bound is **divided** (20a) is read by
        // `ordo_exec::PoolLimits::from_env` at `Pool::connect`, where the
        // fields live. What happens here is the refusal: a library treats
        // an unparseable value as absent, which is right for a library and
        // wrong for a server, where `ORDO_PG_STATEMENT_TIMEOUT_MS=30s`
        // would quietly mean the thirty second default by luck and a
        // misread reserve would quietly mean none.
        let _: u64 = parsed("ORDO_PG_STATEMENT_TIMEOUT_MS", 30_000)?;
        // How long an idle connection is kept (25g), read by
        // `PoolLimits::from_env` and by the invalidation bus for how long a
        // database keeps its listener. Refused here for the reason above.
        let _: u64 = parsed("ORDO_PG_IDLE_SECS", 30)?;
        // Whether a busy store pool also logs Postgres's view of it, read
        // at `assemble_program`. Refused here so a misspelt `of` is not
        // quietly `on`.
        ordo_exec::incident::snapshot_enabled(
            std::env::var(ordo_exec::incident::SNAPSHOT_ENV)
                .ok()
                .as_deref(),
        )
        .map_err(|detail| BootRefusal::Invalid {
            var: ordo_exec::incident::SNAPSHOT_ENV,
            detail,
        })?;
        // How a write that lost a serialization race is tried again, read
        // by `PoolLimits::from_env`, which clamps. Refused here naming the
        // range, so a value out of it is never silently changed.
        let env = |k: &str| std::env::var(k).ok();
        bounded_setting(
            &env,
            ordo_exec::WRITE_RETRIES_ENV,
            u64::from(ordo_exec::PoolLimits::DEFAULT_WRITE_RETRIES),
            0,
            u64::from(ordo_exec::PoolLimits::MAX_WRITE_RETRIES),
        )?;
        bounded_setting(
            &env,
            ordo_exec::WRITE_RETRY_MAX_ENV,
            ordo_exec::PoolLimits::DEFAULT_WRITE_RETRY_MAX.as_millis() as u64,
            1,
            ordo_exec::PoolLimits::MAX_WRITE_RETRY_MAX_MS,
        )?;
        // The door's store pool, read at `assemble_program`. Refused here
        // naming the range.
        store_pool_config(&env, &PoolConfig::from_env())?;
        // The longest `matches` pattern a statement runs, read by
        // `ordo_exec::regex_max_len`, which clamps. Refused here naming
        // the range.
        bounded_setting(
            &env,
            ordo_exec::REGEX_MAX_LEN_ENV,
            ordo_plan::REGEX_MAX_LEN_DEFAULT as u64,
            1,
            ordo_exec::REGEX_MAX_LEN_MAX as u64,
        )?;
        // How many tenant databases the boot pass, the sweeps and a deploy
        // dial at once; read by `ordo_exec::route::AttachedPools::new`,
        // which clamps. Refused here naming the range.
        bounded_setting(
            &env,
            ordo_exec::route::TENANT_DIAL_CONCURRENCY_ENV,
            ordo_exec::route::TENANT_DIAL_CONCURRENCY_DEFAULT as u64,
            1,
            ordo_exec::route::TENANT_DIAL_CONCURRENCY_MAX as u64,
        )?;
        // How long a walker waits for a free dial slot before its step for
        // that tenant fails; read by `AttachedPools::new`, which clamps.
        // Refused here naming the range and the default.
        bounded_setting(
            &env,
            ordo_exec::route::TENANT_DIAL_WAIT_MS_ENV,
            ordo_exec::route::TENANT_DIAL_WAIT_DEFAULT.as_millis() as u64,
            1,
            ordo_exec::route::TENANT_DIAL_WAIT_MAX_MS,
        )?;
        // How long a schema statement waits for its lock, and how the apply
        // tries again (27d, `plan-evolution-plans.md` §3.7): read by
        // `schema_apply::LockPolicy::from_env` where the apply runs.
        // Refused here for the reason above.
        let _: u64 = parsed(
            "ORDO_APPLY_LOCK_TIMEOUT_MS",
            crate::schema_apply::LOCK_TIMEOUT_MS,
        )?;
        let _: u32 = parsed("ORDO_APPLY_LOCK_RETRIES", crate::schema_apply::LOCK_RETRIES)?;
        let _: u64 = parsed(
            "ORDO_APPLY_LOCK_BACKOFF_MS",
            crate::schema_apply::LOCK_BACKOFF_MS,
        )?;
        // Whether `ordo apply` makes safe additive changes itself. Refused
        // here when the value is not a switch, and naming the setting.
        if let Ok(v) = std::env::var(crate::schema_apply::SAFE_CHANGES_SETTING) {
            if crate::schema_apply::safe_changes_value(Some(&v)).is_none() {
                return Err(BootRefusal::Invalid {
                    var: crate::schema_apply::SAFE_CHANGES_SETTING,
                    detail: format!("`{v}` is not on or off; unset it for the default, on"),
                });
            }
        }
        // The shared live stream's idle window and read cap (27s), read by
        // `ordo_api::LiveSettings::from_env` where the door is built.
        // Refused here for the reason above, and a cap of zero refuses too:
        // a stream that admits no read serves nothing.
        let _: u64 = parsed(
            "ORDO_LIVE_IDLE_CLOSE_SECS",
            ordo_api::live::DEFAULT_IDLE_CLOSE_SECS,
        )?;
        let live_reads: usize = parsed(
            "ORDO_LIVE_MAX_READS",
            ordo_api::live::DEFAULT_MAX_READS_PER_STREAM,
        )?;
        if live_reads == 0 {
            return Err(BootRefusal::Invalid {
                var: "ORDO_LIVE_MAX_READS",
                detail: "a live stream that carries no reads serves nothing".into(),
            });
        }
        // How often a quiet feed asks the door again, read by
        // `ordo_api::LiveSettings::from_env`. Outside its bounds refuses:
        // 0 would spin, and past an hour a logout leaves a feed open too
        // long.
        let recheck: u64 = parsed(
            "ORDO_LIVE_RECHECK_SECS",
            ordo_api::live::DEFAULT_RECHECK_SECS,
        )?;
        let (lo, hi) = ordo_api::live::RECHECK_SECS_BOUNDS;
        if !(lo..=hi).contains(&recheck) {
            return Err(BootRefusal::Invalid {
                var: "ORDO_LIVE_RECHECK_SECS",
                detail: format!(
                    "`{recheck}` is outside {lo} to {hi}; unset it for the default, {}",
                    ordo_api::live::DEFAULT_RECHECK_SECS
                ),
            });
        }
        // A refinement's size limits (`refinement-default` ask 6), read by
        // `ordo_api::RefineLimits::from_env` where the door is built.
        // Refused here for the reason above. Zero refuses too: a limit of
        // no conditions, or an `in` of no values, turns the feature off,
        // and refinement has no off switch (ruled 2026-10-05).
        for (var, default) in [
            (
                "ORDO_REFINE_MAX_CONDITIONS",
                ordo_api::refine::DEFAULT_MAX_CONDITIONS,
            ),
            (
                "ORDO_REFINE_MAX_IN_VALUES",
                ordo_api::refine::DEFAULT_MAX_IN_VALUES,
            ),
        ] {
            refine_limit(var, std::env::var(var).ok().as_deref(), default)?;
        }
        // A person's own download (owner ruling 2026-10-22), read by
        // `ordo_api::download::DownloadLimits::from_env` where the door is
        // built. Refused here for the reason above.
        ordo_api::download::DownloadLimits::parse(
            std::env::var("ORDO_OWN_DOWNLOAD_PER_HOUR").ok().as_deref(),
            std::env::var("ORDO_OWN_DOWNLOAD_MAX_BYTES").ok().as_deref(),
        )
        .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
        // A `json` document's size and depth limits (28b), read by
        // `ordo_api::json::limits` where the door is built. Refused here
        // for the reason above; zero refuses too, since a limit of nothing
        // would refuse every document.
        for (var, default) in [
            ("ORDO_JSON_MAX_BYTES", ordo_api::json::DEFAULT_MAX_BYTES),
            ("ORDO_JSON_MAX_DEPTH", ordo_api::json::DEFAULT_MAX_DEPTH),
        ] {
            json_limit(var, std::env::var(var).ok().as_deref(), default)?;
        }
        let reserve: usize = parsed("ORDO_PG_SPACE_RESERVE", 0)?;
        if reserve >= size {
            return Err(BootRefusal::Invalid {
                var: "ORDO_PG_SPACE_RESERVE",
                detail: format!(
                    "a reserve of {reserve} on a pool of {size} leaves nothing for the \
                     space that asked; it is connections held out of ONE space's reach, \
                     not out of the pool"
                ),
            });
        }
        // The password is read by `PoolConfig::from_env` (both the variable
        // and the `_FILE` form), because that is where the field lives and
        // a second reader is a second thing to keep in step.
        let pg = PoolConfig {
            host,
            port,
            user,
            db,
            size,
            acquire_timeout: Duration::from_millis(acquire_ms),
            password: PoolConfig::from_env().password,
        };

        // ── the socket ──
        //
        // Loopback by default: a `cargo run` must not put a door on the
        // network. Exposure is a deliberate line in a manifest.
        let addr_text = std::env::var("ORDO_HOST_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
        let addr: SocketAddr = addr_text.parse().map_err(|_| BootRefusal::Invalid {
            var: "ORDO_HOST_ADDR",
            detail: format!("`{addr_text}` is not an address:port"),
        })?;
        let shutdown_grace = Duration::from_secs(parsed("ORDO_SHUTDOWN_GRACE_SECS", 30u64)?);

        // ── the space ──
        let space = SpaceId(required(
            "ORDO_SPACE",
            "a deployment whose tenant is named `conformance` is not a deployment",
        )?);

        // ── custody ──
        let provider = required(
            "ORDO_KEY_PROVIDER",
            "custody is selected explicitly, never inferred from which other variables are set",
        )?;
        let dev_unsafe = ordo_crypto::master::dev_unsafe_from_env();
        let custody = match provider.trim() {
            // S6-009, ruled 2026-09-28: one HMAC is the whole stretch, so
            // outside development the env master is refused, before the
            // secret is read and before any dial.
            "local" if !dev_unsafe => {
                return Err(BootRefusal::Invalid {
                    var: "ORDO_KEY_PROVIDER",
                    detail: format!("`local`: {}", ordo_crypto::master::env_master_refusal()),
                })
            }
            "local" => {
                // Opened here rather than at construction so the refusal
                // lands before the dial, with every other refusal. The
                // bytes are dropped; `build_custody` opens the source again
                // once the pool exists.
                ordo_crypto::master::MasterKeySource::Env
                    .open()
                    .map_err(BootRefusal::Custody)?;
                CustodyChoice::Local
            }
            "sealed-master" => {
                let path = std::path::PathBuf::from(required(
                    "ORDO_MASTER_KEY_FILE",
                    "`sealed-master` needs the file `ordo keys seal-master` wrote",
                )?);
                let passphrase = std::path::PathBuf::from(required(
                    "ORDO_MASTER_KEY_PASSPHRASE_FILE",
                    "the sealed file and its passphrase are separable on purpose; \
                     neither alone opens anything",
                )?);
                // Opened before the dial too — a wrong passphrase or a
                // missing file should cost milliseconds, not a pool
                // timeout. Argon2id at the pinned profile is ~50ms of the
                // boot, once.
                ordo_crypto::master::MasterKeySource::SealedFile {
                    path: path.clone(),
                    passphrase: passphrase.clone(),
                }
                .open()
                .map_err(BootRefusal::Custody)?;
                CustodyChoice::SealedMaster { path, passphrase }
            }
            "test" if dev_unsafe => CustodyChoice::Test,
            "test" => {
                return Err(BootRefusal::Invalid {
                    var: "ORDO_KEY_PROVIDER",
                    detail: "`test` seals under a master key published in this repository; \
                             set ORDO_DEV_UNSAFE_TEST_KEYS=1 if that is genuinely what you want"
                        .into(),
                })
            }
            other => {
                return Err(BootRefusal::Invalid {
                    var: "ORDO_KEY_PROVIDER",
                    detail: format!("`{other}` is not a provider (local | sealed-master | test)"),
                })
            }
        };

        // ── the tenant credential provider (18d T3, 18e U4) ──
        //
        // Read here, beside custody, because a wrong or malformed mount is
        // the same class of mistake and should cost the same thing: a
        // refused boot in milliseconds rather than a dial that reaches a
        // target it should never have touched. See `ordo_exec::creds`'s
        // module doc for the shape and every rule it enforces.
        let credentials = credentials_from_env()?;

        // ── every tenant has a database of its own (ruled 2026-09-25, 2026-10-06) ──
        //
        // `ORDO_PG_ROUTING` chose between serving every space from the
        // deployment's one database and giving each tenant its own. The
        // first contradicts the ruling, so the setting is gone and the
        // second is the only behaviour. A deployment that still sets it is
        // told: `attached` asked for what it now gets, so it boots with a
        // warning; anything else asked for what no longer exists, so it
        // refuses rather than serve differently from what was configured.
        legacy_routing(std::env::var("ORDO_PG_ROUTING").ok().as_deref())?;
        default_tenant_credential_is_answerable(credentials.is_some())?;

        // ── which store this deployment serves bytes through (18e U5) ──
        //
        // Explicit, and both halves of the argument carry: a deployment that meant `attached` and
        // mistyped gets a refusal rather than a silent demotion to the
        // one bucket, and `attached` with no credential provider refuses
        // because an attachment names a credential label and never a key.
        //
        // The second half is what makes the refusal worth its own line
        // rather than being left to the first upload. Without a provider
        // every attached tenant resolves to a bucket this node holds no
        // material for, so the node would come up, report ready, and fail
        // one upload at a time, and an upload has already consumed the
        // client's bytes by the time it fails, which is §3 B-K3's reason
        // for reading the whole credential directory at boot in the first
        // place.
        //
        let blob_routing = match std::env::var("ORDO_BLOB_ROUTING")
            .unwrap_or_else(|_| "single".into())
            .trim()
        {
            "single" => BlobRouting::Single,
            "attached" if credentials.is_none() => {
                return Err(BootRefusal::Invalid {
                    var: "ORDO_BLOB_ROUTING",
                    detail: "`attached` needs a credential provider: set \
                             ORDO_TENANT_CREDENTIALS=dir and ORDO_TENANT_CREDENTIALS_DIR. \
                             Without one every attached tenant resolves to a bucket this \
                             node holds no key pair for"
                        .into(),
                })
            }
            "attached" => BlobRouting::Attached,
            other => {
                return Err(BootRefusal::Invalid {
                    var: "ORDO_BLOB_ROUTING",
                    detail: format!("`{other}` is not a routing mode (single | attached)"),
                })
            }
        };

        // ── the door's two secrets ──
        let token_secret = ordo_identity::TokenKey::secret_from_env().map_err(|e| match e {
            ordo_identity::TokenKeyError::Missing => BootRefusal::Missing {
                var: "ORDO_TOKEN_KEY",
                why: "a defaulted token key means every deployment built from this source \
                          can mint tokens every other one accepts; set it, or set \
                          ORDO_TOKEN_KEY_FILE to a file that holds it"
                    .into(),
            },
            ordo_identity::TokenKeyError::Malformed => BootRefusal::Invalid {
                var: "ORDO_TOKEN_KEY",
                detail: "must be 64 hex characters (openssl rand -hex 32)".into(),
            },
            ordo_identity::TokenKeyError::File(why) => BootRefusal::Invalid {
                var: "ORDO_TOKEN_KEY_FILE",
                detail: why,
            },
            other => BootRefusal::Invalid {
                var: "ORDO_TOKEN_KEY",
                detail: other.to_string(),
            },
        })?;
        let page_key = PageKey::from_env().map_err(|e| match e {
            ordo_api::PageKeyError::Missing => BootRefusal::Missing {
                var: "ORDO_PAGE_KEY",
                why: "a shared page key makes a cursor minted in one deployment valid in \
                      another; set it, or set ORDO_PAGE_KEY_FILE to a file that holds it"
                    .into(),
            },
            ordo_api::PageKeyError::Malformed => BootRefusal::Invalid {
                var: "ORDO_PAGE_KEY",
                detail: "must be 64 hex characters (openssl rand -hex 32)".into(),
            },
            ordo_api::PageKeyError::File(why) => BootRefusal::Invalid {
                var: "ORDO_PAGE_KEY_FILE",
                detail: why,
            },
            other => BootRefusal::Invalid {
                var: "ORDO_PAGE_KEY",
                detail: other.to_string(),
            },
        })?;

        // ── the signing keys: refuse the FEATURE, not the boot ──
        let checkpoint = ordo_commandlog::sign::Ed25519Signer::from_env().ok();
        let security_checkpoint = ordo_secevent::security_signer_from_env().is_ok();

        // `ORDO_APEX_DOMAIN` is read, recorded, and not acted on. Two
        // documents say the host binary should refuse to start without it;
        // no passkey provider is wired, so a binary that refused over a
        // missing RP id would be refusing on behalf of a feature it does
        // not have. **The refusal belongs to the increment that mounts the
        // passkey endpoints** — a control ships with its producer.
        let apex_domain = std::env::var("ORDO_APEX_DOMAIN").ok();

        // ── CORS ──
        //
        // A list of exact origins, or nothing. A malformed entry and `*`
        // refuse the boot: a mistyped origin that was silently dropped
        // would be discovered by a browser page as a discarded answer,
        // which is the failure 14g exists to end.
        let cors = Self::cors_from_env()?;

        // ── the blob store ──
        //
        // Syntax here; *necessity* at load. See `blob_from_env`.
        let blob = blob_from_env()?;

        // ── the exit ops ──
        //
        // Refuse the FEATURE when unset, like the checkpoint key: a
        // deployment with nowhere to write a bundle serves no export, and
        // the census names the variable. A value that is set and unusable
        // is a refusal of the BOOT, because an operator who set it meant
        // the feature to be on.
        let exit_bundles = exit_bundles_from_env()?;
        let import_bundles = import_bundles_from_env()?;

        // ── the metering ledger (17n): identity and the two cadences ──
        //
        // Both periods are tuning knobs with a stated default, never a
        // refused boot when unset — the same posture `ORDO_PG_POOL_SIZE`
        // takes above. `parsed` already refuses a value that is set and
        // does not parse, which is the owner's ruling: configurable, and a
        // malformed value refuses rather than silently falling back.
        let replica = match std::env::var("ORDO_REPLICA_ID") {
            Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => random_replica_id(),
        };
        let usage_flush_secs: u64 = parsed("ORDO_USAGE_FLUSH_SECS", 60u64)?;
        if usage_flush_secs == 0 {
            return Err(BootRefusal::Invalid {
                var: "ORDO_USAGE_FLUSH_SECS",
                detail: "a zero-second flush never sleeps between ticks".into(),
            });
        }
        // Which platform ops require presence, checked against the ops that
        // exist so a misspelt exemption refuses the boot instead of leaving
        // the op requiring presence while its owner believes it does not.
        let presence = {
            let known: Vec<String> = crate::platform_ops::specs()
                .into_iter()
                .map(|s| s.name)
                .collect();
            let reads =
                crate::presence::reads_from_env().map_err(|detail| BootRefusal::Invalid {
                    var: crate::presence::READS_ENV,
                    detail,
                })?;
            crate::presence::PresencePolicy::from_env(&known)
                .map_err(|detail| BootRefusal::Invalid {
                    var: crate::presence::ENV,
                    detail,
                })?
                .with_reads(reads)
        };
        // Who may pause a tenant from inside it (27ar). A word that is not a
        // role refuses the boot by name.
        let pausing =
            crate::pause::PausePolicy::from_env().map_err(|detail| BootRefusal::Invalid {
                var: crate::pause::PAUSE_ROLES_ENV,
                detail,
            })?;
        // What a paused tenant answers, per reason. A reason word that is
        // not on the list, or a status outside 4xx and 503, refuses the boot.
        let pausing =
            pausing.with_answers(crate::pause::PauseAnswers::from_env().map_err(|detail| {
                BootRefusal::Invalid {
                    var: crate::pause::PAUSE_ANSWERS_ENV,
                    detail,
                }
            })?);
        // Which deletes a delegated token may call (SH-20, 11-4d). Naming a
        // platform op, or writing a pattern, refuses the boot by name.
        let delegable = {
            let platform: Vec<String> = crate::platform_ops::specs()
                .into_iter()
                .map(|s| s.name)
                .collect();
            crate::delegable::DelegablePolicy::from_env(&platform).map_err(|detail| {
                BootRefusal::Invalid {
                    var: crate::delegable::ENV,
                    detail,
                }
            })?
        };
        // The app developer's profile template (27ah). Naming a platform op,
        // a role that does not exist, or a pattern refuses the boot by name.
        let access_template = {
            let platform: Vec<String> = crate::platform_ops::specs()
                .into_iter()
                .map(|s| s.name)
                .collect();
            crate::access_template::AccessTemplate::from_env(&platform).map_err(|detail| {
                BootRefusal::Invalid {
                    var: crate::access_template::ENV,
                    detail,
                }
            })?
        };
        // Recovery's settings, read before anything is dialled: a quarantine
        // longer than the contest window, or a delay below its floor, refuses
        // the boot by name.
        let recovery =
            crate::recovery::RecoveryPolicy::from_env().map_err(|e| BootRefusal::Invalid {
                var: e.var,
                detail: e.detail,
            })?;
        // The second step's settings, refused by name when unknown or out of
        // range, before anything is dialled.
        let second_factor = crate::second_factors::SecondFactorConfig::from_env().map_err(|e| {
            BootRefusal::Invalid {
                var: e.var,
                detail: e.detail,
            }
        })?;
        let usage_rollup_secs: u64 = parsed("ORDO_USAGE_ROLLUP_SECS", 3600u64)?;
        if usage_rollup_secs == 0 {
            return Err(BootRefusal::Invalid {
                var: "ORDO_USAGE_ROLLUP_SECS",
                detail: "a zero-second rollup never sleeps between ticks".into(),
            });
        }

        // ── the authentication surface ──
        //
        // Off unless asked for, and the census says so: a deployment that
        // verifies tokens minted elsewhere is a legitimate shape, and it is
        // what every deployment built from this source was until now.
        let auth = Self::auth_from_env()?;

        // The deployment owners, named on the server (`plan-provisioning-on-
        // signup.md` §3). Parsed here so a malformed id refuses the boot
        // before the dial, like every other setting.
        // With the identity service the setting is not read at all, so it
        // never refuses a boot; the boot log names it (step 8).
        let deployment_owners =
            crate::provision::deployment_owners_from(&|k: &str| std::env::var(k).ok()).map_err(
                |detail| BootRefusal::Invalid {
                    var: crate::provision::DEPLOYMENT_OWNERS_ENV,
                    detail,
                },
            )?;
        let provisioning = crate::self_serve::SelfServeConfig::from_env()
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
        // The mail channel (`plan-auth-federated.md` §7.6). Off unless
        // `ORDO_MAIL_KIND` is set; a setting that is set and wrong refuses
        // the boot, naming the setting and never a secret.
        install_limits(&|k: &str| std::env::var(k).ok())?;
        let mail = crate::mail::MailChannel::from_env()
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
        let sync = sync_pacing_from(&|k: &str| std::env::var(k).ok())?;

        Ok(RuntimeConfig {
            pg,
            blob_routing,
            addr,
            shutdown_grace,
            space,
            custody,
            token_secret,
            page_key,
            checkpoint,
            security_checkpoint,
            apex_domain,
            cors,
            blob,
            exit_bundles,
            import_bundles,
            replica,
            usage_flush: Duration::from_secs(usage_flush_secs),
            usage_rollup: Duration::from_secs(usage_rollup_secs),
            sync,
            auth,
            credentials,
            presence,
            pausing,
            delegable,
            access_template,
            deployment_owners,
            provisioning,
            recovery,
            second_factor,
            mail,
        })
    }
}

/// One line per named deployment owner, so the log says what the boot did.
fn report_deployment_owners(done: Vec<(String, crate::provision::OwnerOutcome)>, home: &str) {
    use crate::provision::OwnerOutcome;
    for (principal, outcome) in done {
        match outcome {
            OwnerOutcome::Added => {
                println!("deployment owner {principal}: added to `{home}` as owner")
            }
            OwnerOutcome::AlreadyOwner => {}
            OwnerOutcome::Kept { role, status } => println!(
                "deployment owner {principal}: already holds a {status} `{role}` row in \
                 `{home}` and it was left alone"
            ),
            OwnerOutcome::Waiting(why) => {
                println!("deployment owner {principal}: not written yet, {why}")
            }
        }
    }
}

/// Which tenant credential provider the deployment configured:
/// `ORDO_TENANT_CREDENTIALS`.
///
/// **Two plans named this differently and only one shipped.**
/// `plan-space-routing.md` §9.1 R-C3 proposed `ORDO_DB_CREDENTIALS`;
/// `plan-tenant-storage.md` §3 B-K1 is the later refinement, arguing that
/// object storage takes the database's provider rather than one of its
/// own, and it renames the selector to `ORDO_TENANT_CREDENTIALS` along the
/// way. The later name is what this binary reads. `ORDO_DB_CREDENTIALS` is
/// not recognized and never was wired.
///
/// **Unset is not a refusal here** (ruled 2026-10-06: configuring nothing
/// must work). Every tenant database on the deployment's own cluster is
/// dialled under `$deployment`, which [`tenant_credentials`] answers with
/// the deployment's own password, so a provider is needed only for a
/// cluster dialled under a label of its own. A deployment that sets it must
/// get a fully working provider or a refusal, never a silent partial one;
/// a deployment that never sets it gets `None`, and every other label is
/// refused at its use, naming this setting.
pub fn credentials_from_env() -> Result<Option<Arc<ordo_exec::creds::Dir>>, BootRefusal> {
    let Ok(spec) = std::env::var("ORDO_TENANT_CREDENTIALS") else {
        return Ok(None);
    };
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(None);
    }
    let bad = |detail: String| BootRefusal::Invalid {
        var: "ORDO_TENANT_CREDENTIALS",
        detail,
    };
    match spec {
        "dir" => {
            let dir = required(
                "ORDO_TENANT_CREDENTIALS_DIR",
                "the `dir` provider reads one mounted directory; there is no default \
                 mount point to fall back to",
            )?;
            let provider = ordo_exec::creds::Dir::open(std::path::Path::new(&dir)).map_err(bad)?;
            Ok(Some(Arc::new(provider)))
        }
        other => Err(bad(format!("`{other}` is not a provider (dir)"))),
    }
}

/// `ORDO_REPLICA_ID`'s default: eight random hex characters, drawn fresh
/// per process. For an identifier that must not repeat across replicas,
/// not a key — [`ordo_crypto::fill_random`]'s own posture.
fn random_replica_id() -> String {
    let mut bytes = [0u8; 4];
    ordo_crypto::fill_random(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `ORDO_EXIT_BUNDLE_DIR`: unset is `None`; set must be an absolute path.
///
/// Absolute because a relative one resolves against whatever directory the
/// process was started in, and an export bundle holds a space's whole
/// contents: where it lands should not depend on how the binary was
/// launched. Whether the directory can be created is
/// checked at assembly, since that is I/O.
fn exit_bundles_from_env() -> Result<Option<std::path::PathBuf>, BootRefusal> {
    let Ok(raw) = std::env::var("ORDO_EXIT_BUNDLE_DIR") else {
        return Ok(None);
    };
    let path = std::path::PathBuf::from(raw.trim());
    if raw.trim().is_empty() || !path.is_absolute() {
        return Err(BootRefusal::Invalid {
            var: "ORDO_EXIT_BUNDLE_DIR",
            detail: format!(
                "`{raw}` is not an absolute path; export bundles must land somewhere that \
                 does not depend on the directory the binary was started in"
            ),
        });
    }
    Ok(Some(path))
}

/// `ORDO_IMPORT_BUNDLE_DIR`: unset is `None`; set must be an absolute path.
///
/// Absolute for `exit_bundles_from_env`'s reason, and one more that is
/// sharper on this side: this directory is an **input**, so a relative path
/// would make which bytes a deployment imports depend on the directory the
/// binary was started in. Whether it exists is checked at assembly.
fn import_bundles_from_env() -> Result<Option<std::path::PathBuf>, BootRefusal> {
    let Ok(raw) = std::env::var("ORDO_IMPORT_BUNDLE_DIR") else {
        return Ok(None);
    };
    let path = std::path::PathBuf::from(raw.trim());
    if raw.trim().is_empty() || !path.is_absolute() {
        return Err(BootRefusal::Invalid {
            var: "ORDO_IMPORT_BUNDLE_DIR",
            detail: format!(
                "`{raw}` is not an absolute path; which bundle a deployment imports must \
                 not depend on the directory the binary was started in"
            ),
        });
    }
    Ok(Some(path))
}

/// What a leftover `ORDO_PG_ROUTING` means at boot (ruled 2026-10-06).
///
/// Unset is the ordinary case. `attached` asked for what every deployment
/// now does, so it is a warning. Any other value, `single` above all,
/// asked for a deployment that serves tenants from one database, which no
/// longer exists, and it refuses: booting anyway would serve differently
/// from what the deployment owner wrote down.
pub fn legacy_routing(value: Option<&str>) -> Result<(), BootRefusal> {
    match value.map(str::trim) {
        None | Some("") => Ok(()),
        Some("attached") => {
            eprintln!(
                "  ORDO_PG_ROUTING is set and the setting is gone: every tenant has a \
                 database of its own, which is now the only behaviour. Remove the line"
            );
            Ok(())
        }
        Some(other) => Err(BootRefusal::Invalid {
            var: "ORDO_PG_ROUTING",
            detail: format!(
                "`{other}`: the setting is gone. Every tenant has a database of its own \
                 (ruled 2026-09-25 and 2026-10-06), so no deployment serves its tenants \
                 from one database. Remove the line; ORDO_PG_TENANT_HOST and its siblings \
                 choose the cluster tenants' databases go on"
            ),
        }),
    }
}

/// A default tenant cluster that names a credential label of its own needs
/// somewhere to read it from.
///
/// `ORDO_PG_TENANT_CREDENTIAL` unset means `$deployment`, which the server
/// answers with its own password, so the zero-configuration deployment
/// needs no credential provider. A label of the deployment owner's own
/// choosing is read from `ORDO_TENANT_CREDENTIALS`, and without one every
/// tenant creation would fail at its dial. Refused here instead, where the
/// fix is one line.
fn default_tenant_credential_is_answerable(provider: bool) -> Result<(), BootRefusal> {
    let label = std::env::var("ORDO_PG_TENANT_CREDENTIAL").unwrap_or_default();
    let label = label.trim();
    if provider || label.is_empty() || label == ordo_exec::route::OWN_CREDENTIAL {
        return Ok(());
    }
    Err(BootRefusal::Invalid {
        var: "ORDO_PG_TENANT_CREDENTIAL",
        detail: format!(
            "names the label `{label}` and no credential provider is configured to answer \
             it: set ORDO_TENANT_CREDENTIALS=dir and ORDO_TENANT_CREDENTIALS_DIR, or leave \
             ORDO_PG_TENANT_CREDENTIAL unset to dial with the deployment's own password"
        ),
    })
}

/// The credential provider every tenant database is dialled with.
///
/// The label `$deployment` always answers, with the deployment's own
/// password (`own`), so a deployment that configured no credential
/// provider still gives every tenant a database on its own cluster. Any
/// other label is read from the credentials directory when there is one,
/// and refused naming `ORDO_TENANT_CREDENTIALS` when there is not.
pub fn tenant_credentials(
    dir: Option<Arc<ordo_exec::creds::Dir>>,
    own: Option<String>,
) -> Arc<dyn ordo_exec::route::DbCredentials> {
    let inner: Arc<dyn ordo_exec::route::DbCredentials> = match dir {
        Some(dir) => dir,
        None => Arc::new(ordo_exec::route::Unconfigured),
    };
    ordo_exec::route::WithOwnCredential::wrapping(inner, own)
}

/// The credential provider `BlobRouting::Attached` signs to buckets with.
///
/// [`tenant_credentials`]' twin, over the same
/// [`ordo_exec::creds::Dir`] and through the other of its two traits. One
/// provider answering both questions is §3 B-K1's whole point: two would
/// be two rotation procedures, two boot refusals and two label lists on
/// one node.
///
/// Fallible in type for that function's stated reason: an embedder who
/// hand-builds a `RuntimeConfig` gets the refusal rather than a resolver
/// that signs as nobody. It is unreachable with `None` from the ordinary
/// path, which `RuntimeConfig::from_env` already refused.
fn bucket_credentials_for_routing(
    cfg: &RuntimeConfig,
) -> Result<Arc<dyn ordo_exec::creds::BucketCredentials>, BootRefusal> {
    match &cfg.credentials {
        Some(dir) => Ok(dir.clone()),
        None => Err(BootRefusal::Invalid {
            var: "ORDO_BLOB_ROUTING",
            detail: "`attached` needs a credential provider and this runtime was built \
                     without one"
                .into(),
        }),
    }
}

/// The platform ops this deployment registers, from what it has wired.
///
/// `PlatformOps` requires that an op with no state behind it is
/// **unregistered**, so the door answers for it as it does for any name
/// nobody registered, rather than registered and failing at call time.
///
/// - `std.space.callersOf`: always. Its state is the access engine, the
///   grant registry and the delegation registry, which every deployment has.
/// - the exit ops: when `ORDO_EXIT_BUNDLE_DIR` is set.
/// - the attestation ops: when the checkpoint signer is, which is the
///   attestation worker's own enabling condition.
/// - `std.space.attachBucket`: **when `ORDO_BLOB_ROUTING=attached` and
///   this deployment has both a store and a credential provider** (18p
///   U6). Three blockers closed in turn, and the order they closed in is
///   the argument for the condition:
///
///   1. No configuration for the credential material. Closed 2026-09-22 by
///      T3/U4's provider (`ordo_exec::creds::Dir`), which holds labels for
///      buckets as well as databases.
///   2. A blob tier that resolved every space to one `SingleStore` which
///      does not read attachments — and that blocker was the *disclosure*
///      rather than the plumbing. `attach` writes a custody event and
///      returns the sentence a consent page prints, which says the
///      tenant's objects live in the tenant's bucket. While every upload
///      resolved to the deployment's one store that sentence was false,
///      and a false custody disclosure is worse than an op a caller cannot
///      reach: one is a missing feature and the other is a promise the
///      platform is not keeping. U5's `AttachedStores` closed it.
///   3. The op itself: the refusals, the attach-time probe that proves the
///      credential signs, and the custody event. 18p U6.
///
///   So the condition is not "U6 shipped" but "this node honours an
///   attachment". On `single` every upload still resolves to the one
///   `ORDO_BLOB_STORE`, so registering the op there would put the
///   disclosure of (2) back. See [`buckets_wired`], which is where the
///   condition is written; the census says which part is missing on the
///   node it is printed on.
/// - `std.deployment.apply` (18l): always. Its state is the control pool
///   and the deployment's own space name, both of which every deployment
///   has by the time it is assembled — and a deployment that could not be
///   deployed to is the state 17t's readiness ruling says a platform must
///   not be in.
fn platform_specs(exit: bool, imports: bool, attest: bool, buckets: bool) -> Vec<ordo_api::OpSpec> {
    use crate::{apply_op, attest_ops, bucket_ops, exit_ops, platform_ops};
    platform_ops::specs()
        .into_iter()
        .filter(|s| match s.name.as_str() {
            platform_ops::CALLERS_OF => true,
            // A person's own tenants (owner, 2026-09-29): wired by nothing
            // beyond the engine, so it is served wherever platform ops are.
            platform_ops::MY_TENANTS => true,
            apply_op::APPLY => true,
            // G1 to G3, wired with `APPLY`'s state.
            name if crate::deploy_ops::OPS.contains(&name) => true,
            // 17aa piece 3 and 11-15: wired unconditionally in `assemble`
            // (`with_tenants`, `with_reap`), and until 2026-09-29 missing
            // here, so the binary answered 404 for both.
            crate::tenant_ops::CREATE | crate::reap_op::REAP => true,
            exit_ops::EXPORT | exit_ops::DELETE => exit,
            exit_ops::IMPORT => imports,
            name if attest_ops::OPS.contains(&name) => attest,
            bucket_ops::ATTACH_BUCKET | bucket_ops::DETACH_BUCKET => buckets,
            // E3: the membership commands are served wherever platform
            // ops are, and are wired unconditionally in `platform_ops_for`.
            name if crate::member_ops::OPS.contains(&name) => true,
            // 27aq: leaving a tenant, wired beside the membership commands.
            name if crate::release_ops::OPS.contains(&name) => true,
            // E4 and E5: admission policy, first-comer included.
            crate::admission::SET_ADMISSION | crate::admission::ADMISSION => true,
            // A tenant's own CORS origins. Always registered: off, they
            // answer with the setting that switches them on.
            name if crate::tenant_origins::OPS.contains(&name) => true,
            // E6: the password give and the reset.
            name if crate::credential_ops::OPS.contains(&name) => true,
            // G2: sharing between tenants.
            name if crate::share_ops::OPS.contains(&name) => true,
            // L1 to L3: open share links.
            name if crate::share_links::OPS.contains(&name) => true,
            // 24e: tenant invitations.
            name if crate::invite_ops::OPS.contains(&name) => true,
            // P2: the self-serve provisioning setting.
            name if crate::self_serve::OPS.contains(&name) => true,
            // Observed op cost, read from the metrics registry every
            // assembly has.
            name if crate::observed_cost::OPS.contains(&name) => true,
            // G14: console keys. Always registered: with no sign-in they
            // answer with the setting that switches it on.
            name if crate::console_keys::OPS.contains(&name) => true,
            // Backend functions: the app secrets, in the control database
            // every assembly has.
            name if crate::secrets::OPS.contains(&name) => true,
            // Backend functions: the runs and their logs, in the control
            // database every assembly has.
            name if crate::fn_runs::OPS.contains(&name) => true,
            // G4: the list of tenants, read from the registry every
            // assembly has.
            name if crate::fleet::OPS.contains(&name) => true,
            // G8: the deployment's security record, in the control
            // database every assembly has.
            name if crate::security_record::OPS.contains(&name) => true,
            // G7: the deployment's settings, read from this process and
            // the control database every assembly has.
            name if crate::settings::OPS.contains(&name) => true,
            // G9: the quotas, in the control database every assembly has.
            name if crate::quotas::OPS.contains(&name) => true,
            // §7A: the guest sign-in setting.
            name if crate::auth::guest::OPS.contains(&name) => true,
            // How long sessions last, in three layers.
            name if crate::session_policy::OPS.contains(&name) => true,
            // The password slowdown, in three layers.
            name if crate::guess_limits::OPS.contains(&name) => true,
            // Whether a person must have a verified email address.
            name if crate::contact_email::OPS.contains(&name) => true,
            // Recovery R5: the deployment owner's recovery and succession
            // ops. Registered wherever platform ops are, and refused with
            // a sentence where recovery is not configured, as E6's are.
            name if crate::identity_ops::OPS.contains(&name) => true,
            // 27ar: pausing a tenant, wired unconditionally beside reap.
            name if crate::pause::OPS.contains(&name) => true,
            // L1: a tenant's own sign-in providers. Always registered: off,
            // they answer with the setting that switches them on.
            name if crate::tenant_providers::OPS.contains(&name) => true,
            // 11-4b: the consent ceremony. Always registered: with no agent
            // named in `ORDO_DELEGATION_AGENTS` no request is accepted, so
            // there is nothing for the five ops to find.
            name if crate::consent::OPS.contains(&name) => true,
            // 27ah: a tenant's own profile and groups, wired beside the
            // membership commands.
            name if crate::profile_ops::OPS.contains(&name) => true,
            // G5: who is in a tenant and what they may do, read from the
            // membership state wired beside the profile ops.
            name if crate::people_ops::OPS.contains(&name) => true,
            _ => false,
        })
        .collect()
}

/// Does this configuration honour a bucket attachment?
///
/// The one place the condition is written, read by [`platform_specs`], by
/// [`census`] and by the wiring in [`assemble`], so the op cannot be
/// registered without its state or wired without being registered — which
/// is the invariant `PlatformOps` requires and the one a second spelling of
/// this condition would let drift.
///
/// All three parts are load-bearing: `attached` because on `single` an
/// attachment is recorded and never honoured; a store because
/// `AttachedStores` resolves a space with no row to the deployment's own
/// and there has to be one; a credential provider because an attachment
/// names a label and a node holding none can sign for nothing.
fn buckets_wired(cfg: &RuntimeConfig) -> bool {
    matches!(cfg.blob_routing, BlobRouting::Attached)
        && cfg.blob.is_some()
        && cfg.credentials.is_some()
}

// ── the blob store ───────────────────────────────────────────────────────

/// Where a deployment's blob bytes go: `ORDO_BLOB_STORE`.
///
/// Two forms, and the selector is explicit for the same reason
/// `ORDO_KEY_PROVIDER` is — a deployment that meant S3 and mistyped must
/// get a refusal, never a silent demotion to a directory on the pod's
/// ephemeral disk:
///
/// ```text
/// ORDO_BLOB_STORE=s3            # binding from ORDO_BLOB_S3_* (S3Config::from_env)
/// ORDO_BLOB_STORE=fs:/var/lib/ordo/blobs
/// ```
///
/// **`fs:` needs `ORDO_DEV_UNSAFE_BLOB_STORE=1`.** `FsStore`'s own module
/// doc asks for exactly this and says why: a filesystem has no durability
/// story anybody has written down, so it is a test double and *"a
/// deployment configuration naming it outside dev mode refuses to boot with
/// the reason, exactly as `LocalKeyProvider::from_env` refuses a default
/// master key."* The authority is [`ordo_blob::BlobStore::production_ready`]
/// rather than the scheme string, so a future binding that answers `false`
/// inherits the rule without anybody remembering to add it.
///
/// **A second dev flag rather than reusing `ORDO_DEV_UNSAFE_TEST_KEYS`.**
/// They unsafe different things and a deployment can legitimately want one
/// without the other: a self-host laptop running a real local master key is
/// not a deployment that agreed to put customer files on a filesystem. One
/// flag unlocking both would hand an operator who asked for the first the
/// second in silence, which is the failure the explicit selector exists to
/// prevent.
///
/// **Unset is not a refusal here.** Whether a store is *needed* is a
/// question about the program — see [`blob_needed`] — and the program lives
/// in the database. This function decides syntax and credentials, both pure
/// environment reads, so a typo still refuses in milliseconds.
pub fn blob_from_env() -> Result<Option<BlobBinding>, BootRefusal> {
    let Ok(spec) = std::env::var("ORDO_BLOB_STORE") else {
        return Ok(None);
    };
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(None);
    }
    let bad = |detail: String| BootRefusal::Invalid {
        var: "ORDO_BLOB_STORE",
        detail,
    };
    let (store, label, own): (
        Arc<dyn ordo_blob::BlobStore>,
        String,
        Option<crate::bucket_ops::OwnStore>,
    ) = match spec {
        "s3" => {
            let cfg = ordo_blob::S3Config::from_env().map_err(|e| bad(e.to_string()))?;
            let label = format!("s3 {}/{}", cfg.endpoint.host, cfg.bucket);
            let own = crate::bucket_ops::OwnStore::at_host(&cfg.endpoint.host, &cfg.bucket);
            (ordo_blob::S3Store::shared(cfg), label, Some(own))
        }
        other => match other.strip_prefix("fs:") {
            Some(path) if !path.is_empty() => {
                let store = ordo_blob::FsStore::shared(path).map_err(|e| bad(e.to_string()))?;
                (store, format!("fs {path}"), None)
            }
            _ => return Err(bad(format!("`{other}` is not a store (s3 | fs:<path>)"))),
        },
    };
    if !store.production_ready()
        && !matches!(
            std::env::var("ORDO_DEV_UNSAFE_BLOB_STORE").as_deref(),
            Ok("1" | "true" | "yes")
        )
    {
        return Err(bad(format!(
            "`{spec}` is a test double with no durability story — customer files would live \
             on it; set ORDO_DEV_UNSAFE_BLOB_STORE=1 if that is genuinely what you want"
        )));
    }
    let label = if store.production_ready() {
        label
    } else {
        format!("{label}  (UNSAFE — test double)")
    };
    Ok(Some(BlobBinding { store, label, own }))
}

/// Does this program need a blob store?
///
/// **Ruled (2026-09-07): a deployment must configure a blob store exactly
/// when its loaded program declares a `blob` field that is not `inline`.**
/// Three cases, and the rule falls out of what each one can actually do:
///
/// - **No blob field at all.** Every blob route addresses `entity.field`
///   and `PlatformBlobs` refuses a field that is not a blob, so no upload
///   is reachable. A required store would be a dependency nothing can
///   touch, and it would make every deployment of every program that
///   existed before blobs need a bucket.
/// - **`blob inline` only.** Those bytes ride the row's sealed payload and
///   never reach the object store — that is what makes the tier GC-free by
///   inheritance. Requiring a bucket for bytes that never leave Postgres
///   would be requiring the wrong thing.
/// - **A non-inline `blob` field.** The declaration is a promise the schema
///   already made: the column exists, `std.E.create`/`.update` accept the
///   field, the interface descriptor publishes it, and a generated client
///   renders a file input. With no store the first upload is a 500 in
///   production on the path the program was written for. This is not
///   "refuse the feature and say so" — the platform is not coherent without
///   it, because the program has already said the field is there.
pub(crate) fn blob_needed(schema: &ordo_schema::Schema) -> Option<String> {
    for e in &schema.entities {
        for f in &e.fields {
            if matches!(
                f.ty,
                ordo_schema::FieldTy::Scalar(ordo_syntax::ScalarTy::Blob)
            ) && !f.inline
            {
                return Some(format!("{}.{}", e.name, f.name));
            }
        }
    }
    None
}

/// One `Ed25519Signer` behind two consumers.
///
/// `PgAttestation` takes a `Box<dyn Signer>` and the checkpoint and
/// attestation workers need the same key — a receiver holding the published
/// verifying key must be able to check both, and two signers read from the
/// same environment would be two chances to read it differently. The signer
/// itself is deliberately not `Clone` (it holds the seed), so sharing is an
/// `Arc` and a forwarding impl rather than a copy.
struct SharedSigner(Arc<ordo_commandlog::Ed25519Signer>);

impl ordo_commandlog::Signer for SharedSigner {
    fn key_id(&self) -> &str {
        self.0.key_id()
    }
    fn sign(&self, bytes: &[u8]) -> Vec<u8> {
        self.0.sign(bytes)
    }
    fn verify(&self, bytes: &[u8], signature: &[u8]) -> bool {
        self.0.verify(bytes, signature)
    }
}

// ── the door, rebuildable ────────────────────────────────────────────────

/// The access checker the binary mounts: [`crate::IdentityAccess`] behind a
/// lock, so a program swap can install a freshly built one.
///
/// A swap changes what the ops *are*, and the engine deploys a manifest pin
/// per op — a capability grant pins, per op, the manifest it consented to,
/// so a program whose op discloses more must kill the grant rather than
/// inherit it. A door built once at boot would keep pinning the manifests
/// of a version it stopped serving, which is the quiet direction: the
/// grants that should have died are the ones that survive.
///
/// The recorder is **shared across rebuilds**, because the security drain
/// holds the one this door writes denials into. A rebuilt door with a fresh
/// recorder would be a door whose denials reach no database at all.
pub struct SwappableAccess {
    inner: RwLock<Arc<crate::IdentityAccess>>,
    /// **The spaces whose members could not be read when this door was
    /// built** (18d T5), as `(space, reason)`.
    ///
    /// Held beside the door because it is a property *of that door*: the
    /// engine serving right now was built without those tenants' members,
    /// so no request naming one is allowed until a rebuild. A verified
    /// caller is answered `server_busy`, not `access_denied`
    /// ([`AccessChecker::behind`], reported fix
    /// pool-exhaustion-reads-as-denied). A rebuild replaces both together, which is the same
    /// argument that put the recorder and the deny list here.
    ///
    /// Read by `/health/ready`, which merges it with the resolver's
    /// quarantine register so an operator sees **one** per-tenant list.
    /// Before T5 this lived only in a line printed at boot, so a tenant
    /// the door could not build was invisible to every surface an
    /// operator actually watches.
    unread: RwLock<Vec<(String, String)>>,
    /// **The spaces this door serves from their last good member list**
    /// because their database could not be read for a passing reason
    /// (owner, 2026-10-27), as `(space, note)`. Replaced with the door, as
    /// [`Self::unread`] is, and shown beside it on `/health/ready`.
    kept_stale: RwLock<Vec<(String, String)>>,
    /// **The spaces a rebuild for a change in them failed to install**
    /// (reported fix tenant-access-lags-after-join). The door serving now
    /// predates that change, so a person just admitted is not in it. Each is
    /// answered `server_busy` rather than `access_denied`
    /// ([`AccessChecker::behind`]) until a rebuild installs. Emptied by
    /// every rebuild that installs: its build took in every newer read.
    failed: RwLock<std::collections::BTreeSet<String>>,
    /// The one recorder, held so a rebuild can reinstall it.
    pub recorder: Arc<ordo_secevent::Recorder>,
    /// The one session deny list, held for the same reason and a sharper
    /// one: a rebuilt door without it is a door that **forgets every
    /// logout** the moment somebody deploys. A revoked session would come
    /// back with the next `ordo apply`, which is the quiet direction: the
    /// deployment looks healthy and the tokens somebody ended are honoured
    /// again.
    pub revocations: Option<Arc<dyn ordo_identity::session::SessionRevocations>>,
    /// The second-step levels, reinstalled on every door a rebuild makes for
    /// the reason the deny list is: a rebuilt door that forgot them would stop
    /// asking a tenant's requirement of the very next session.
    second_step: std::sync::OnceLock<Arc<crate::second_factors::SecondStepLevels>>,
    /// The deployment's own space, given to every door installed.
    console_home: std::sync::OnceLock<String>,
    /// The identity service's keys, given to every door installed.
    identity_keys: std::sync::OnceLock<Arc<crate::identity_keys::IdentityKeys>>,
    /// The last read of every tenant database, behind the lock that makes
    /// rebuilds one at a time. See [`crate::directory::Cache`].
    directory: tokio::sync::Mutex<crate::directory::Cache>,
    /// The own tenants being set up, as `TenantOps` holds them, so a call
    /// into one by the person it is for is answered `tenant_not_ready`.
    /// Set once by the runtime; unset, nobody is told it.
    not_ready: std::sync::OnceLock<Arc<crate::tenant_setup::NotReady>>,
    /// Who has a verified email address, and which ops the app developer
    /// requires one for ([`crate::contact_email`]). Set once; unset,
    /// nothing is required.
    contact: std::sync::OnceLock<Arc<crate::contact_email::ContactEmails>>,
}

impl SwappableAccess {
    /// Give the door the email requirement. Once.
    pub fn wire_contact(&self, contact: Arc<crate::contact_email::ContactEmails>) {
        let _ = self.contact.set(contact);
    }

    /// Give the door the own tenants being set up. Once.
    pub fn wire_not_ready(&self, held: Arc<crate::tenant_setup::NotReady>) {
        let _ = self.not_ready.set(held);
    }

    /// Give the door that is serving, and every door installed after it, the
    /// second-step levels. Once.
    pub fn wire_second_step(&self, levels: Arc<crate::second_factors::SecondStepLevels>) {
        if self.second_step.set(levels.clone()).is_ok() {
            self.current().wire_second_step(levels);
        }
    }

    /// Give the door that is serving, and every door installed after it,
    /// the identity service's keys. Once.
    pub fn wire_identity_keys(&self, keys: Arc<crate::identity_keys::IdentityKeys>) {
        if self.identity_keys.set(keys.clone()).is_ok() {
            self.current().wire_identity_keys(keys);
        }
    }

    /// Give the door that is serving, and every door installed after it,
    /// the deployment's own space. Once.
    pub fn wire_console_home(&self, home: &str) {
        if self.console_home.set(home.to_string()).is_ok() {
            self.current().wire_console_home(home);
        }
    }

    pub fn new(door: crate::IdentityAccess) -> SwappableAccess {
        SwappableAccess {
            second_step: std::sync::OnceLock::new(),
            console_home: std::sync::OnceLock::new(),
            identity_keys: std::sync::OnceLock::new(),
            recorder: door.stream.clone(),
            revocations: door.revocations.clone(),
            inner: RwLock::new(Arc::new(door)),
            unread: RwLock::new(Vec::new()),
            kept_stale: RwLock::new(Vec::new()),
            failed: RwLock::new(Default::default()),
            directory: tokio::sync::Mutex::new(crate::directory::Cache::default()),
            not_ready: std::sync::OnceLock::new(),
            contact: std::sync::OnceLock::new(),
        }
    }

    /// What the door that is serving was built without. See the field.
    pub fn unread(&self) -> Vec<(String, String)> {
        self.unread.read().expect("door lock").clone()
    }

    /// The spaces served from their last good member list. See the field.
    pub fn kept_stale(&self) -> Vec<(String, String)> {
        self.kept_stale.read().expect("door lock").clone()
    }

    /// Set how long a space's last good member list is served through a
    /// passing failure ([`crate::directory::STALE_GRACE_SETTING`]). Zero
    /// denies at once. Called at boot, before any rebuild.
    pub async fn set_stale_grace(&self, grace: std::time::Duration) {
        let mut cache = self.directory.lock().await;
        let _ = std::mem::replace(&mut *cache, crate::directory::Cache::with_grace(grace));
    }

    /// Tests only: the door's cache fails every fresh tenant read as a
    /// passing (`Some(true)`) or lasting (`Some(false)`) error, or stops.
    pub async fn force_read_failure(&self, passing: Option<bool>) {
        self.directory.lock().await.force_failure(passing);
    }

    /// Tests only: move the door's cache clock forward.
    pub async fn advance_cache_clock(&self, by: std::time::Duration) {
        self.directory.lock().await.advance(by);
    }

    /// Record what the door about to be installed could not read.
    ///
    /// Separate from [`Self::install`] rather than folded into it,
    /// because `install` is also reached by callers that build a door
    /// from a single pool and have nothing to say here, and a parameter
    /// they all had to pass as empty would be a parameter somebody
    /// eventually passes wrongly.
    fn note_unread(&self, unread: Vec<(String, String)>) {
        *self.unread.write().expect("door lock") = unread;
    }

    pub fn current(&self) -> Arc<crate::IdentityAccess> {
        self.inner.read().expect("door lock").clone()
    }

    fn install(&self, door: crate::IdentityAccess) {
        if let Some(levels) = self.second_step.get() {
            door.wire_second_step(levels.clone());
        }
        if let Some(home) = self.console_home.get() {
            door.wire_console_home(home);
        }
        if let Some(keys) = self.identity_keys.get() {
            door.wire_identity_keys(keys.clone());
        }
        *self.inner.write().expect("door lock") = Arc::new(door);
    }
}

/// Every method forwards to the door that is serving. The token gate and
/// `token_known` are forwarded explicitly: the trait gives both a default
/// (pass, and known), and a wrapper that leaves them to the default is a
/// door that never refuses a withdrawn delegation or a dead share grant,
/// while looking exactly like one that does. `tests/delegation.rs` pins the
/// gate through this type.
#[async_trait::async_trait]
impl AccessChecker for SwappableAccess {
    fn check(&self, token: &str, space: &str, op: &str) -> AccessDecision {
        self.current().check(token, space, op)
    }

    async fn token_gate(&self, token: &str) -> ordo_api::TokenGate {
        self.current().token_gate(token).await
    }

    fn token_known(&self, token: &str) -> bool {
        self.current().token_known(token)
    }

    fn caller_kind(&self, token: &str) -> ordo_api::CallerKind {
        self.current().caller_kind(token)
    }

    fn record_refusal(
        &self,
        token: Option<&str>,
        space: Option<&str>,
        op: &str,
        why: ordo_api::DoorRefusal,
    ) {
        self.current().record_refusal(token, space, op, why)
    }

    fn anonymous_space(&self) -> Option<String> {
        self.current().anonymous_space()
    }

    fn presence_required(&self, token: &str, space: &str, op: &str) -> bool {
        self.current().presence_required(token, space, op)
    }

    /// A tenant still being set up answers its own person `tenant_not_ready`
    /// (owner, 2026-10-21). Anyone else, a share grant and a delegation
    /// included, keeps the door's uniform denial.
    /// The member list for `space` is behind its database: its last read
    /// failed ([`Self::unread`]) or a rebuild for a change in it failed
    /// ([`Self::failed`]). Answered for a verified, live token only, so a
    /// caller with no valid token keeps the uniform denial.
    fn behind(&self, token: &str, space: &str) -> bool {
        let listed = self
            .unread
            .read()
            .expect("door lock")
            .iter()
            .any(|(s, _)| s == space)
            || self.failed.read().expect("door lock").contains(space);
        if !listed {
            return false;
        }
        let door = self.current();
        let now = ordo_identity::Timestamp((door.now)());
        let Ok(claims) = ordo_identity::verify(&door.key, token, now) else {
            return false;
        };
        if let (Some(session), Some(revocations)) =
            (claims.session.as_ref(), door.revocations.as_ref())
        {
            if revocations.is_revoked(&session.id)
                || revocations.principal_ended(&claims.principal, session.started_at)
            {
                return false;
            }
        }
        true
    }

    fn not_ready(&self, token: &str, space: &str) -> Option<u64> {
        let held = self.not_ready.get()?;
        let wanted = held.principal_of(space)?;
        let door = self.current();
        let now = ordo_identity::Timestamp((door.now)());
        let claims = ordo_identity::verify(&door.key, token, now).ok()?;
        if claims.capability.is_some() || claims.delegation.is_some() {
            return None;
        }
        if let (Some(session), Some(revocations)) =
            (claims.session.as_ref(), door.revocations.as_ref())
        {
            if revocations.is_revoked(&session.id)
                || revocations.principal_ended(&claims.principal, session.started_at)
            {
                return None;
            }
        }
        (claims.principal.0 == wanted).then(|| held.retry_secs())
    }

    /// A signed-in person without a verified email address, calling an op
    /// the app developer requires one for (owner ruling 2026-10-22). A
    /// share link's or a delegated token is never asked: it is not the
    /// person signing in.
    async fn email_required(
        &self,
        token: &str,
        _principal: &str,
        op: &str,
    ) -> Option<ordo_api::EmailGate> {
        let contact = self.contact.get()?;
        let door = self.current();
        let now = ordo_identity::Timestamp((door.now)());
        let claims = ordo_identity::verify(&door.key, token, now).ok()?;
        if claims.capability.is_some() || claims.delegation.is_some() || claims.session.is_none() {
            return None;
        }
        // The token's person, never a member id the engine resolved.
        contact.gate(&claims.principal.0, op).await
    }
}

/// Build the door over one loaded program.
///
/// **Reads every tenant's members from its own database**
/// (`plan-space-routing.md` §9.3 D-2), through [`crate::directory::routed_engine`]
/// rather than [`crate::deployment_engine_pooled`]: an attached tenant's
/// founding member lives in its target, per the `membership-home` ruling,
/// and a door built from the control pool alone cannot see it, which would
/// deny every request in that tenant with a door working exactly as
/// written.
///
/// **Credentials are the real provider, wired at 18d T3 / 18e U4.**
/// `credentials` is `cfg.credentials` from [`RuntimeConfig::from_env`]: a
/// [`ordo_exec::creds::Dir`] when the operator set
/// `ORDO_TENANT_CREDENTIALS=dir`, or nothing, wrapped either way by
/// [`tenant_credentials`] so `$deployment` answers with the deployment's own
/// password. A target `$spacedb` names is reached only if its label
/// answers on this node; a label this node does not hold fails at the
/// credential lookup before a dial is even attempted, the same "reported,
/// not fatal" path an unreachable target takes (see
/// [`crate::directory::Directory::unread`], printed below).
#[allow(clippy::too_many_arguments)]
async fn door_for(
    fixture: &Fixture,
    pool: &Pool,
    token_secret: &[u8; 32],
    metrics: &Arc<Metrics>,
    recorder: Arc<ordo_secevent::Recorder>,
    delegations: Arc<crate::delegation::PgDelegationRegistry>,
    revocations: Option<Arc<dyn ordo_identity::session::SessionRevocations>>,
    credentials: &Arc<dyn ordo_exec::route::DbCredentials>,
) -> Result<(crate::IdentityAccess, Vec<(String, String)>), BootRefusal> {
    let template = pool.config();
    let directory = crate::directory::routed_engine(fixture, pool, credentials.as_ref(), &template)
        .await
        .map_err(BootRefusal::Engine)?;
    Ok(door_from(
        directory,
        pool,
        token_secret,
        metrics,
        recorder,
        delegations,
        revocations,
    ))
}

/// The door over a directory already read: [`door_for`]'s second half, and
/// what [`refresh_access`] builds from its cache.
fn door_from(
    directory: crate::directory::Directory,
    pool: &Pool,
    token_secret: &[u8; 32],
    metrics: &Arc<Metrics>,
    recorder: Arc<ordo_secevent::Recorder>,
    delegations: Arc<crate::delegation::PgDelegationRegistry>,
    revocations: Option<Arc<dyn ordo_identity::session::SessionRevocations>>,
) -> (crate::IdentityAccess, Vec<(String, String)>) {
    let unread_spaces = directory.unread_spaces.clone();
    // A target that could not be read denies only its own spaces (they get
    // no members and the door refuses them); the deployment still comes up.
    // Named here rather than swallowed, because "the directory is
    // incomplete" must not be silent: see `directory.rs`'s module doc.
    for line in &directory.unread {
        eprintln!("door: {}", with_password_hint(line));
    }
    let engine = directory.engine;
    let mut door = crate::IdentityAccess::sharing(
        Arc::new(engine),
        ordo_identity::TokenKey::from_secret(token_secret),
        Box::new(now_secs),
        metrics.clone(),
    )
    .with_delegations(delegations);
    // G5: the door asks the grant registry whether a share grant is live.
    door = door.with_shares(Arc::new(crate::grants::PgGrantRegistry::new(pool.clone())));
    // The **same** deny list the routes write, so a logout the routes
    // recorded is a logout this door refuses. Two lists would be a logout
    // nothing hears about.
    if let Some(r) = revocations {
        door = door.with_revocations(r);
    }
    // The recorder the drain is already draining. Assigned rather than
    // constructed, for the reason `SwappableAccess` states.
    door.stream = recorder;
    (door, unread_spaces)
}

/// What [`platform_ops_for`] builds from. Every field is a piece the runtime
/// already holds for the door or the blob tier; none is built a second time.
struct PlatformWiring<'a> {
    /// The metrics registry, which holds observed op cost and is what
    /// `std.deployment.observedCost` reads.
    metrics: &'a Arc<Metrics>,
    /// This replica's name, which `std.deployment.observedCost` gives with
    /// its numbers.
    replica: &'a str,
    /// The session policy layers, shared with the sign-in routes. `None`
    /// when the deployment has no authentication.
    session_policy: Option<Arc<crate::session_policy::SessionPolicies>>,
    /// The password slowdown's layers, shared with the sign-in routes.
    /// `None` when the deployment has no authentication.
    guess_limits: Option<Arc<crate::guess_limits::GuessLimits>>,
    /// A person's verified email address and the requirement, shared with
    /// the sign-in routes and the door. `None` without authentication.
    contact: Option<Arc<crate::contact_email::ContactEmails>>,
    /// Console keys, shared with the sign-in routes (G14). `None` with no
    /// sign-in.
    console_keys: Option<Arc<crate::console_keys::ConsoleKeys>>,
    /// The app secrets (backend functions), shared with [`Runtime::secrets`].
    secrets: &'a Arc<crate::secrets::SecretStore>,
    /// What a completed credential reset does to the person's second factors.
    redeem_hook: crate::credential_ops::RedeemHook,
    /// The second-step levels the admission op writes a tenant's choice to.
    second_levels: &'a Arc<crate::second_factors::SecondStepLevels>,
    pool: &'a Pool,
    keys: &'a Arc<dyn KeyProvider>,
    access: &'a Arc<SwappableAccess>,
    delegations: &'a Arc<crate::delegation::PgDelegationRegistry>,
    /// **The deployment's one pool resolver** (18d T2), handed in rather
    /// than built here. `exit_ops.rs`' own doc has said since it was
    /// written that it should be given `Spaces::pools()`: export, erasure
    /// and a chain proof each reach a space's rows without going through
    /// `Spaces::resolve`, so a resolver of their own is a second answer to
    /// which database those rows are in. An export that read the control
    /// database for a tenant attached elsewhere would produce an empty
    /// bundle and call it a disclosure.
    pools: &'a Arc<dyn ordo_exec::route::PoolResolver>,
    /// The space resolver itself, for a platform op that writes through the
    /// protocol into a tenant's chain (a leave, 27aq) and so needs the
    /// tenant's executor and chain head, not only its pool.
    spaces: &'a Arc<crate::spaces::Spaces>,
    exit_bundles: Option<std::path::PathBuf>,
    /// `std.space.import`'s staging root, present exactly when the op is
    /// registered.
    import_bundles: Option<std::path::PathBuf>,
    /// The blob door the op path holds, handed on rather than rebuilt, so an
    /// import's uploads are the same uploads the HTTP blob route makes
    /// (`plan-blob-chain.md` C1-C8: one upload path).
    blob_door: Option<Arc<dyn ordo_exec::import::BlobDoor>>,
    attest: bool,
    /// The deployment's own space, which is the only space
    /// `std.deployment.apply` answers in. See `crate::apply_op`'s module
    /// doc for the three checks that stack behind that op and for why this
    /// one is a name rather than `is_system_space`.
    home: SpaceId,
    stores: Option<Arc<dyn ordo_blob::StoreResolver>>,
    pipeline: Option<Arc<ordo_blob::BlobPipeline>>,
    /// The tenants' own CORS origins, which `std.space.setOrigins` writes.
    origins: Arc<crate::tenant_origins::TenantOriginMap>,
    /// The deployment whose schema a new tenant's database receives
    /// (18d T8). `None` in the paths that assemble platform ops before a
    /// deployment exists; `std.space.create` and sign-up then make no
    /// tenant, and say so, rather than make one with nowhere to serve it.
    serving: Option<&'a Arc<Deployment>>,
    /// The credential **labels** this node holds, for a target's dial.
    credentials: &'a Arc<dyn ordo_exec::route::DbCredentials>,
    /// `std.space.attachBucket`'s state (18p U6), present exactly when
    /// [`buckets_wired`] says the op is registered. Built in [`assemble`]
    /// rather than here, because it needs the bucket half of the credential
    /// provider and the deployment's own store identity, and both are
    /// read where `cfg` is still whole — the same reason
    /// `bucket_credentials_for_routing` is called there.
    buckets: Option<Arc<crate::bucket_ops::BucketOps>>,
    /// The membership commands rebuild the door on this replica when they
    /// change who is in a space (E3), which needs what `AccessRefresh`
    /// needs. `None` where no deployment is serving, and then a command
    /// announces and leaves this replica to its timer.
    members: Option<MemberWiring<'a>>,
    /// Recovery, filled in once the authentication state has built it. The
    /// deployment owner's recovery ops hold this and refuse until it is set.
    recovery: crate::identity_ops::RecoveryHandle,
    /// The deployment's recovery settings.
    recovery_policy: crate::recovery::RecoveryPolicy,
    /// What the environment says about self-serve provisioning (P2).
    provisioning: crate::self_serve::SelfServeConfig,
    /// Runs the acl rows of an accepted share as the sharer (G3).
    acl: Option<Arc<dyn crate::share_accept::AclRunner>>,
}

/// What the membership commands need beyond the pieces every op shares.
struct MemberWiring<'a> {
    token_secret: [u8; 32],
    metrics: &'a Arc<Metrics>,
    deployment: &'a Arc<Deployment>,
    /// A directory reader, so an admission names a principal that exists.
    directory: Option<Arc<dyn ordo_identity::DirectoryStore>>,
    /// Tickets, for the invitation way in (E4). Present exactly when the
    /// deployment has authentication, because a ticket is redeemed by a
    /// person who is signing in.
    tickets: Option<Arc<crate::tickets::Tickets>>,
    /// The ways in a tenant on this deployment may switch on: what
    /// `ORDO_AUTH_PROVISIONING` now means.
    ceiling: crate::admission::Ceiling,
    /// The credential ops' state (E6). Present exactly when the deployment
    /// has authentication.
    credentials: Option<CredentialWiring>,
}

/// What the password give and the reset need that only authentication has.
struct CredentialWiring {
    store: Arc<dyn ordo_identity::CredentialStore>,
    revocations: Arc<dyn ordo_identity::session::SessionRevocations>,
    session_max_secs: u64,
    allow_temporary: bool,
}

/// The platform ops' state, wired from the runtime's own pieces.
///
/// **The engine is read through the door, on every call.** A program swap
/// installs a rebuilt `IdentityAccess` with a new engine, so the engine
/// handed to `PlatformOps::new` is only the boot's; the engine source
/// follows `SwappableAccess::current` and is what `callersOf` answers from.
///
/// What is wired here must match [`platform_specs`]: state for exactly the
/// ops registered, and nothing for the rest.
async fn platform_ops_for(
    w: PlatformWiring<'_>,
) -> Result<Arc<crate::platform_ops::PlatformOps>, BootRefusal> {
    // The grant registry `callersOf`'s token column reads. Consent writes
    // land in the same table; the door never consults it to admit a request.
    let grants = Arc::new(crate::grants::PgGrantRegistry::new(w.pool.clone()));
    grants
        .ensure_table()
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;

    let share_grants: Arc<dyn ordo_identity::grants::GrantRegistry> = grants.clone();
    let release_grants: Arc<dyn ordo_identity::grants::GrantRegistry> = grants.clone();
    let consent_grants: Arc<dyn ordo_identity::grants::GrantRegistry> = grants.clone();
    let live = w.access.clone();
    let now: Arc<dyn Fn() -> u64 + Send + Sync> = Arc::new(now_secs);
    let stream = w.access.recorder.clone();
    let pools = w.pools.clone();
    let delegations: Arc<dyn ordo_identity::delegation::DelegationRegistry> = w.delegations.clone();

    let mut ops = crate::platform_ops::PlatformOps::new(
        w.access.current().engine.clone(),
        grants,
        now.clone(),
    )
    .with_engine_source(Arc::new(move || live.current().engine.clone()))
    .with_delegations(delegations);

    if let Some(bundles) = w.exit_bundles {
        ops = ops.with_exit(Arc::new(crate::exit_ops::ExitOps {
            pool: w.pool.clone(),
            pools: pools.clone(),
            keys: w.keys.clone(),
            policy: ordo_exec::cadence::LogPolicy::DEFAULT,
            bundles,
            imports: w.import_bundles.clone(),
            door: w.blob_door.clone(),
            stream: Some(stream.clone()),
            blobs: w.pipeline,
            stores: w.stores,
            now: now.clone(),
        }));
    }
    // 18p U6. Present exactly when `platform_specs` registered the op, so
    // the `PlatformOps` invariant holds in both directions: no registered
    // op without state, and no state for an op nobody can call.
    if let Some(buckets) = w.buckets {
        ops = ops.with_buckets(buckets);
    }
    if w.attest {
        ops = ops.with_attest(Arc::new(crate::attest_ops::AttestOps {
            pool: w.pool.clone(),
            pools,
            policy: ordo_exec::cadence::LogPolicy::DEFAULT,
            stream: Some(stream.clone()),
            mint_secret: Arc::new(|| {
                let mut secret = vec![0u8; 32];
                ordo_crypto::fill_random(&mut secret);
                secret
            }),
            now: now.clone(),
        }));
    }
    // 18l. Always, matching `platform_specs`: the state is the control
    // pool and the deployment's own space, and every assembled deployment
    // has both.
    // T7 R-S3: the deploy door applies to every database this deployment
    // serves from, through the same resolver everything else resolves
    // through, and holds the activation back unless every one took it.
    ops = ops.with_apply(crate::apply_op::ApplyOps::recording(
        w.pool.clone(),
        w.pools.clone(),
        &w.home.0,
        Some(stream.clone()),
    ));
    // 17aa piece 3. Always, for `with_apply`'s reason and one more: the
    // `KeyProvider` is here already (`ExitOps` took it above), and a
    // deployment that registered `std.space.create` and wired nothing for
    // it would accept a tenant and create nothing.
    // 18d T8: the routing half, when the caller has one to give. This is
    // what gives `provision_space_on` and `prepare_target` their first
    // production caller — until now nothing outside a test called either,
    // so no deployment put any tenant on a database of its own.
    let tenants = match w.serving {
        Some(serving) => crate::tenant_ops::TenantOps::with_routing(
            w.pool.clone(),
            w.keys.clone(),
            &w.home.0,
            crate::tenant_ops::Routing {
                serving: serving.clone(),
                // Answers `$deployment` with the deployment's own password,
                // which is what lets a deployment that installed no
                // credential directory still give every tenant a database
                // on its own cluster. See [`tenant_credentials`].
                credentials: w.credentials.clone(),
                // The shape, not an address: the address is on the
                // attachment. Sizes and timeouts come from this
                // deployment's own configuration so a tenant's pool is
                // built like every other.
                template: ordo_exec::PoolConfig::from_env(),
                // R-X3, read once here at the edge rather than inside the
                // check. See `provision::HostAllowlist`.
                // Plus the two hosts this deployment configured itself,
                // which D-S6 makes allowed by definition: *"if i defined it
                // on my platform it is allowed."* Otherwise an unset
                // `ORDO_PG_ATTACH_HOSTS` would refuse a deployment its own
                // default tenant cluster, and no tenant could be created
                // until an operator named their own server back to them.
                hosts: crate::provision::HostAllowlist::from_env()
                    .allowing(&w.pool.config().host)
                    .allowing(&crate::tenant_ops::TenantCluster::from_env(&w.pool.config()).host),
                // Level 3, read once at the edge for the same reason.
                // Defaults to this deployment's own cluster, so a
                // deployment that configures nothing still gives every
                // tenant a database.
                default_cluster: crate::tenant_ops::TenantCluster::from_env(&w.pool.config()),
                stream: Some(stream.clone()),
            },
        ),
        None => crate::tenant_ops::TenantOps::new(w.pool.clone(), w.keys.clone(), &w.home.0),
    };
    ops = ops.with_tenants(tenants.clone());

    // E3: the membership commands. Always wired, because they are always
    // registered. The refresh hook rebuilds the door on this replica so a
    // suspension is refused on the next request, and `MemberOps` announces
    // on `ordo.members` so every other replica does the same.
    {
        let (refresh, directory) = match &w.members {
            Some(m) => {
                let (deployment, pool, metrics) =
                    (m.deployment.clone(), w.pool.clone(), m.metrics.clone());
                let (access, delegations) = (w.access.clone(), w.delegations.clone());
                let (credentials, secret) = (w.credentials.clone(), m.token_secret);
                let hook: crate::member_ops::Refresh = Arc::new(move |space: &str| {
                    let (deployment, pool, metrics) =
                        (deployment.clone(), pool.clone(), metrics.clone());
                    let (access, delegations) = (access.clone(), delegations.clone());
                    let (credentials, secret) = (credentials.clone(), secret);
                    let space = space.to_string();
                    Box::pin(async move {
                        refresh_access_for(
                            &deployment,
                            &pool,
                            &secret,
                            &metrics,
                            &access,
                            delegations,
                            &credentials,
                            &space,
                        )
                        .await
                        .map_err(|e| e.to_string())
                    })
                });
                (Some(hook), m.directory.clone())
            }
            None => (None, None),
        };
        // 25e: a tenant made here is served by this replica's door before
        // its maker is answered.
        if let Some(hook) = &refresh {
            tenants.refresh_with(hook.clone());
        }
        // Owner, 2026-10-21: the setup settings, and the door's view of the
        // own tenants being set up.
        tenants.set_setup(w.provisioning.setup);
        w.access.wire_not_ready(tenants.not_ready());
        let members = Arc::new(crate::member_ops::MemberOps {
            control: w.pool.clone(),
            pools: w.pools.clone(),
            directory,
            stream: Some(stream.clone()),
            refresh,
            now: now.clone(),
            managed: w
                .members
                .as_ref()
                .and_then(|m| m.credentials.as_ref())
                .map(|_| crate::credential_ops::ManagedStore::new(w.pool.clone())),
        });
        // E4: admission policy, under the deployment's ceiling. Its table is
        // the deployment's own record about a tenant, so it is created here
        // with the others rather than by an operator command.
        crate::admission::ensure_table(w.pool)
            .await
            .map_err(|e| BootRefusal::Database(e.to_string()))?;
        // The tenants' second-step levels, read now that their table exists.
        // Unread is a deployment whose tenants require nothing yet.
        let _ = w.second_levels.load(w.pool).await;
        // The connections each tenant accepts a two-step claim from. Unread
        // is no tenant accepting any, the safe direction.
        if let Some(acceptance) = crate::presence::upstream() {
            let _ = crate::admission::load_presence_from(w.pool, &acceptance).await;
        }
        // The claim code was removed (owner, 2026-10-30). A deployment that
        // still sets it boots; the log says it is ignored and names the
        // first-comer setting. Its stored row is left for the later
        // release's drop migration.
        if let Some(note) =
            crate::admission::ignored_claim_code_note(&|k: &str| std::env::var(k).ok())
        {
            eprintln!("  {note}");
        }
        // P2: the self-serve provisioning setting and its limits. The
        // environment seeds them on a boot that finds nothing stored.
        crate::self_serve::ensure_table(w.pool)
            .await
            .map_err(|e| BootRefusal::Database(e.to_string()))?;
        let self_serve = crate::self_serve::SelfServeStore::new(w.pool.clone());
        for note in self_serve
            .boot(&w.provisioning, now())
            .await
            .map_err(|detail| BootRefusal::Invalid {
                var: crate::self_serve::MODE_ENV,
                detail: detail.to_string(),
            })?
        {
            eprintln!("  {note}");
        }
        ops = ops.with_observed_cost(Arc::new(crate::observed_cost::ObservedCostOps {
            costs: w.metrics.clone(),
            home: w.home.0.clone(),
            node: w.replica.to_string(),
        }));
        ops = ops.with_self_serve(Arc::new(crate::self_serve::SelfServeOps {
            store: self_serve,
            home: w.home.0.clone(),
            now: now.clone(),
            stream: Some(stream.clone()),
        }));
        if let Some(p) = w.session_policy.clone() {
            ops = ops.with_session_policy(p);
        }
        if let Some(g) = w.guess_limits.clone() {
            ops = ops.with_guess_limits(g);
        }
        if let Some(c) = w.contact.clone() {
            ops = ops.with_contact(c);
        }
        ops = ops.with_secrets(Arc::new(crate::secrets::SecretOps {
            store: w.secrets.clone(),
            now: now.clone(),
            stream: Some(stream.clone()),
        }));
        ops = ops.with_fn_runs(Arc::new(crate::fn_runs::FnRunOps {
            control: w.pool.clone(),
            home: w.home.0.clone(),
        }));
        if let Some(keys) = w.console_keys.clone() {
            ops = ops.with_console_keys(Arc::new(crate::console_keys::ConsoleKeyOps {
                keys,
                revocations: w.access.revocations.clone(),
                now: now.clone(),
                stream: Some(stream.clone()),
            }));
        }
        ops = ops.with_guest(Arc::new(crate::auth::guest::GuestOps {
            store: crate::auth::guest::GuestStore::new(w.pool.clone()),
            home: w.home.0.clone(),
            now: now.clone(),
            stream: Some(stream.clone()),
        }));
        ops = ops.with_admission(Arc::new(crate::admission::Admission {
            members: members.clone(),
            store: crate::admission::PolicyStore::new(w.pool.clone())
                .with_levels(w.second_levels.clone()),
            tickets: w.members.as_ref().and_then(|m| m.tickets.clone()),
            ceiling: w
                .members
                .as_ref()
                .map(|m| m.ceiling.clone())
                .unwrap_or_else(crate::admission::Ceiling::invitation_only),
            home: Some(w.home.0.clone()),
        }));
        // E6: the password give and the reset, where there are credentials.
        if let Some(c) = w.members.as_ref().and_then(|m| m.credentials.as_ref()) {
            crate::credential_ops::ensure_table(w.pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            let m = w.members.as_ref().expect("credentials imply members");
            if let (Some(directory), Some(tickets)) = (m.directory.clone(), m.tickets.clone()) {
                ops = ops.with_credentials(Arc::new(crate::credential_ops::CredentialOps {
                    members: members.clone(),
                    directory,
                    credentials: c.store.clone(),
                    hasher: Arc::new(ordo_identity::Argon2idHasher::new()),
                    tickets,
                    managed: crate::credential_ops::ManagedStore::new(w.pool.clone()),
                    revocations: c.revocations.clone(),
                    session_max_secs: c.session_max_secs,
                    allow_temporary: c.allow_temporary,
                    control: w.pool.clone(),
                    after_redeem: w.redeem_hook.clone(),
                }));
                // E7: what deleting a tenant does to the people it made.
                // Wired where `$managed` exists, which is where credentials
                // are.
                ops = ops.with_identities(Arc::new(crate::tenant_identities::TenantIdentities {
                    control: w.pool.clone(),
                    connections: Arc::new(crate::tenant_identities::NoConnections),
                    revocations: Some(c.revocations.clone()),
                    session_max_secs: c.session_max_secs,
                    stream: Some(stream.clone()),
                    now: now.clone(),
                }));
            }
        }
        // G2: sharing between tenants, where tickets exist (authentication).
        if let Some(tickets) = w.members.as_ref().and_then(|m| m.tickets.clone()) {
            // 24e: invitations, on the same tickets.
            ops = ops.with_invite(Arc::new(crate::invite_ops::InviteOps {
                tickets: tickets.clone(),
                stream: Some(stream.clone()),
                now: now.clone(),
            }));
            let max_secs =
                crate::share_ops::max_secs_from_env().map_err(|detail| BootRefusal::Invalid {
                    var: crate::share_ops::MAX_SECS_ENV,
                    detail,
                })?;
            ops = ops.with_share(Arc::new(crate::share_ops::ShareOps {
                tickets,
                stream: Some(stream.clone()),
                now: now.clone(),
                max_secs,
                grants: share_grants,
                members: members.clone(),
                accept: w.acl.clone().map(|acl| {
                    let live = w.access.clone();
                    crate::share_accept::AcceptWiring {
                        engine: Arc::new(move || live.current().engine.clone()),
                        acl,
                    }
                }),
            }));
            // L1 to L3: open share links, on the same share state.
            if let Some(share) = ops.share.clone() {
                let settings = crate::share_links::LinkSettings::from_env()
                    .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
                crate::share_links::ensure_table(&share.members.control)
                    .await
                    .map_err(|e| BootRefusal::Database(e.to_string()))?;
                ops = ops.with_links(Arc::new(
                    crate::share_links::ShareLinks::new(share, settings)
                        .with_home(w.home.0.clone()),
                ));
            }
        }
        // Recovery R5: the deployment owner's recovery and succession ops.
        // They read the person's directory, change a tenant's owner through
        // the membership commands' own store, and reach recovery through the
        // handle the authentication state fills.
        // A tenant's part (R6): who it created, and what it has chosen about
        // holding a recovered person. `$managed` exists where credentials do.
        crate::recovery_hold::ensure_table(w.pool)
            .await
            .map_err(|e| BootRefusal::Database(e.to_string()))?;
        let managed = w
            .members
            .as_ref()
            .and_then(|m| m.credentials.as_ref())
            .map(|_| crate::credential_ops::ManagedStore::new(w.pool.clone()));
        let grants_for_succession = ops.grants.clone();
        ops = ops.with_identity(Arc::new(crate::identity_ops::IdentityOps {
            recovery: w.recovery.clone(),
            directory: w.members.as_ref().and_then(|m| m.directory.clone()),
            members: members.clone(),
            home: w.home.0.clone(),
            managed,
            tenants: Some(crate::recovery_hold::TenantRecovery::new(
                w.pool.clone(),
                w.recovery_policy.hold_default,
            )),
            // Completing a succession ends the departed owners' delegations
            // and their grants to apps, through the consent records.
            consents: Some(crate::consent::Consents {
                pool: w.pool.clone(),
                delegations: w.delegations.clone(),
                grants: grants_for_succession,
            }),
        }));
        // 27aq: leaving a tenant reads the release rule of the program being
        // served, on every call, and writes through the protocol into the
        // tenant's chain.
        // The name a leaver's seat keeps is the app developer's choice
        // (owner, 2026-10-06): kept unless `ORDO_MEMBER_LEFT_NAME` says
        // otherwise. A value that cannot be used refuses the boot by name.
        let left_name =
            crate::release_ops::left_name_from_env().map_err(|detail| BootRefusal::Invalid {
                var: crate::release_ops::LEFT_NAME_ENV,
                detail,
            })?;
        ops = ops.with_release(Arc::new(crate::release_ops::ReleaseOps {
            members: members.clone(),
            spaces: w.spaces.clone(),
            grants: Some(release_grants),
            left_name,
        }));
        // 27ah: a tenant's own profile and groups, written into the tenant's
        // chain through the same resolver an op call uses.
        {
            let live = w.access.clone();
            ops = ops.with_profiles(Arc::new(crate::profile_ops::ProfileOps {
                spaces: w.spaces.clone(),
                members: members.clone(),
                engine: Arc::new(move || live.current().engine.clone()),
            }));
        }
        ops = ops.with_members(members);
    }

    // 27ar: pausing a tenant. Always wired: the gate reads the register on
    // every op call, and the register is loaded here from the registry so a
    // tenant paused before this boot is refused from the first request.
    crate::pause::reload(w.spaces, w.pool)
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;
    // G7: the deployment's settings, from this process's variables (each
    // secret's value dropped before it is held) and the stored ones.
    ops = ops.with_settings(Arc::new(crate::settings::SettingsOps::from_vars(
        w.pool.clone(),
        w.home.0.clone(),
        std::env::vars().collect(),
    )));
    // G4: the list of tenants, beside pausing them.
    ops = ops.with_tenant_list(Arc::new(crate::fleet::TenantListOps {
        control: w.pool.clone(),
        home: w.home.0.clone(),
        spaces: Some(w.spaces.clone()),
    }));
    // G8: the deployment's own security record, which the security drain
    // writes to the control database.
    ops = ops.with_security_record(Arc::new(crate::security_record::SecurityRecordOps {
        control: w.pool.clone(),
        home: w.home.0.clone(),
    }));
    // G9: the quotas, enforced when the usage ledger is on.
    ops = ops.with_quotas(Arc::new(crate::quotas::QuotaOps {
        control: w.pool.clone(),
        home: w.home.0.clone(),
        enforced: w.metrics.usage_enabled(),
        stream: Some(stream.clone()),
        now: now.clone(),
    }));
    ops = ops.with_pause(Arc::new(crate::pause::PauseOps {
        control: w.pool.clone(),
        spaces: w.spaces.clone(),
        home: w.home.0.clone(),
        stream: Some(stream.clone()),
        now: now.clone(),
    }));

    // 11-15: `std.space.reap`. Always wired, for `with_tenants`' reason —
    // a deployment that creates tenants can leave a failed one behind, and
    // the op that releases it must not depend on configuration the failure
    // might itself have been about.
    // 11-4b: the consent ceremony, over the same registries the door and
    // `callersOf` read. Its settings refuse the boot by name.
    let settings = crate::consent::Settings::from_env()
        .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
    crate::consent::Ceremony::ensure_table(w.pool)
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;
    {
        let live = w.access.clone();
        ops = ops.with_consent(Arc::new(crate::consent::Ceremony {
            pool: w.pool.clone(),
            delegations: w.delegations.clone(),
            grants: consent_grants,
            engine: Arc::new(move || live.current().engine.clone()),
            spaces: w.spaces.clone(),
            stream: Some(stream.clone()),
            now: now.clone(),
            settings,
        }));
    }

    ops = ops.with_reap(Arc::new(crate::reap_op::ReapOps {
        pool: w.pool.clone(),
        keys: w.keys.clone(),
        home: w.home.0.clone(),
        credentials: w.credentials.clone(),
        template: ordo_exec::PoolConfig::from_env(),
        stream: Some(stream.clone()),
        now: now.clone(),
    }));

    ops = ops.with_origins(w.origins.clone());
    Ok(Arc::new(ops))
}

/// Wall clock in seconds since the epoch: the one the door, the auth
/// routes and the Postgres-backed stores all read.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── assembly ─────────────────────────────────────────────────────────────

/// A deployment, assembled and not yet listening.
pub struct Runtime {
    /// What the startup log prints for custody. A label, never material.
    pub custody_label: &'static str,
    /// Recorded, printed, and acted on by nothing — see
    /// [`RuntimeConfig::from_env`].
    pub apex_domain: Option<String>,
    /// The CORS allowlist the door is wrapped in; see [`door`].
    pub cors: Option<ordo_api::cors::CorsPolicy>,
    /// The tenants' own CORS origins, layered on `cors` by [`door`].
    pub tenant_origins: Arc<crate::tenant_origins::TenantOriginMap>,
    /// Proxy mode's pacer (`crate::proxy`). `None` is proxy mode off: no
    /// decision uses a client address.
    pub pacer: Option<Arc<crate::proxy::Pacer>>,
    pub cfg_addr: SocketAddr,
    pub shutdown_grace: Duration,
    pub space: SpaceId,
    /// What this boot found, or did, about the system space. Printed,
    /// because a space the platform created without being asked is exactly
    /// the kind of thing an operator should read in the log rather than
    /// discover in a table.
    pub system_space: crate::provision::SystemSpace,
    pub pg: PoolConfig,
    pub pool: Pool,
    /// The control database's record of every signed head
    /// (`restore-witness`, R2d). Over [`Runtime::pool`], which *is* the
    /// control pool: `assemble` boots with it and `AttachedPools::control`
    /// is built from it. The checkpoint worker writes through this handle
    /// and the boot reconciliation reads it.
    pub witness: ordo_exec::witness::Witness,
    /// What the boot's reconciliation found, per space (17aa). Printed by
    /// [`Runtime::print_startup`]; the live answer for as long as the
    /// process runs is the resolver's quarantine register, which
    /// `/health/ready` reads.
    pub reconciled: Vec<(String, crate::spaces::Reconciled)>,
    pub keys: Arc<dyn KeyProvider>,
    /// The tenant credential provider (18d T3, 18e U4): a real
    /// [`ordo_exec::creds::Dir`] when `ORDO_TENANT_CREDENTIALS=dir`, or the
    /// empty [`ordo_exec::route::Installed`] when nothing was configured.
    /// Held so a deploy or a member-added rebuild ([`refresh_access`],
    /// [`spawn_deploy_listener`]) reuses the boot's own provider rather than
    /// re-reading the environment and the mount a second time.
    pub credentials: Arc<dyn ordo_exec::route::DbCredentials>,
    pub deployment: Arc<Deployment>,
    pub access: Arc<SwappableAccess>,
    pub metrics: Arc<Metrics>,
    pub api: ApiState,
    pub platform: PlatformState,
    pub secdrain: SecurityDrain,
    /// Whether `ORDO_SECURITY_CHECKPOINT_KEY` is set: the enabling
    /// condition of the `secevent.checkpoint` and `secevent.retain` workers
    /// (R3).
    pub security_checkpoint: bool,
    /// Kept for the swap path, which writes the deployment owners on the
    /// first `apply` of a deployment that booted against an empty database.
    deployment_owners: Vec<String>,
    /// Kept for the swap path, which rebuilds the door.
    token_secret: [u8; 32],
    delegations: Arc<crate::delegation::PgDelegationRegistry>,
    /// The invalidation bus, kept concrete so that a tenant created after
    /// boot can be listened to (`PgInvalidationBus::refresh`).
    pg_bus: Arc<crate::invalidation::PgInvalidationBus>,
    /// The census the startup log prints and `serve` re-prints nothing of.
    pub disabled: Vec<(String, String)>,
    /// What the startup log prints for the authentication surface. `None`
    /// is a deployment that verifies tokens somebody else minted, which is
    /// what every deployment built from this source was before **20b**.
    pub auth_label: Option<String>,
    /// What the startup log prints for passkeys: the relying-party id and
    /// how many origins may run a ceremony. `None` when `passkey` is not a
    /// method.
    pub passkey_label: Option<String>,
    /// What recovery settled on, for the startup log. Present with
    /// authentication, since recovery is a way back into a sign-in.
    pub recovery_label: Option<String>,
    /// What the second step settled on, for the startup log. Present with
    /// authentication.
    pub second_step_label: Option<String>,
    /// The tenants' second-step levels, re-read on a timer so another replica
    /// learns of a level an owner set here.
    pub second_levels: Arc<crate::second_factors::SecondStepLevels>,
    /// The mail channel, when `ORDO_MAIL_KIND` is set. Its outbox table
    /// exists from boot, and `mail.send` drains it.
    pub mail: Option<crate::mail::MailChannel>,
    /// What the startup log prints for the blob tier. `None` is "no store
    /// configured", which is legitimate for a program with no stored blob
    /// field and refused at load for one that has any.
    pub blob_label: Option<String>,
    /// The two blob routes' state, when a store is configured.
    pub blobs: Option<ordo_api::blob::BlobState>,
    /// `GET /download`'s state: a person's own download
    /// ([`crate::own_download`]).
    pub download: ordo_api::download::DownloadState,
    /// The resolver the sweep (R3) and the exit ops share.
    pub stores: Option<Arc<dyn ordo_blob::StoreResolver>>,
    /// What the supervisor will run (R3): the live program's routines
    /// (checklist 12a, [`crate::routines::wiring`]), or what a test set with
    /// [`Runtime::with_triggers`]. Empty for a program with no routines.
    pub triggers: TriggerWiring,
    /// The routines in force now, which the `triggers.*` workers read and
    /// the deploy listener switches with the program (no restart).
    pub routines: Arc<crate::routines::LiveRoutines>,
    checkpoint_policy: ordo_exec::cadence::LogPolicy,
    checkpoint_signer: Option<Arc<ordo_commandlog::Ed25519Signer>>,
    /// This process's identity in `$usage_replica` (17n). Printed at
    /// startup so an operator reading the log can tell replicas apart in
    /// `$usage_replica` without a second lookup.
    pub replica: String,
    /// `usage.flush`'s cadence (17n M1b).
    pub usage_flush: Duration,
    /// `usage.rollup`'s cadence (17n M1c).
    pub usage_rollup: Duration,
    /// The per-space pool resolver `usage.rollup` (M1c) and the sweep (R3)
    /// both walk. A local of `assemble` until M1c needed a caller outside
    /// it able to reach every target, not only the space this process
    /// booted with.
    pub spaces: Arc<crate::spaces::Spaces>,
    /// Backend functions (`POST /fn/<name>`): the serving version's table,
    /// reloaded on every deploy. The wasmtime engine is made on the first
    /// call. Outbound calls go through the egress kernel over
    /// [`Runtime::secrets`]; logs are scrubbed of those secrets' values.
    /// the egress kernel is wired in.
    pub functions: Arc<crate::functions::HostFunctions>,
    /// Brings a tenant's database to the version this node serves
    /// (25i). Shared by the `tenant.catchup` worker and the deploy
    /// listener, which nudges it when this node swaps to a new version.
    pub catchup: Arc<crate::tenant_forward::CatchUp>,
    /// This node's row in `$nodes`: what it serves and whether the tenant
    /// catch-up for it has finished. `ordo apply` waits on it.
    pub node_status: Arc<crate::node_status::NodeStatus>,
    /// The same [`crate::quotas::PgQuotas`] `rt.api.quota` holds, kept as its
    /// concrete type too (17n M2) — `usage.flush`'s [`Worker::run`] needs
    /// [`crate::quotas::PgQuotas::refresh`], which is not part of the
    /// `QuotaGate` trait object `ApiState.quota` erases to. `None` when the
    /// ledger is off, the same posture `usage_flush`/`usage_rollup`'s
    /// workers take.
    pub quotas: Option<Arc<crate::quotas::PgQuotas>>,
    /// The app secrets for backend functions (`plan-backend-functions.md`
    /// §5), bound by the serving program. The function runtime's `fetch` is
    /// [`crate::secrets::fn_fetch`] over this store.
    pub secrets: Arc<crate::secrets::SecretStore>,
}

/// The trigger surface a deployment registers.
///
/// Built from the program's `routine` declarations by
/// [`crate::routines::wiring`] (checklist 12a), or supplied by a test. A
/// program with no routines gets the empty wiring and every trigger worker
/// boots `DISABLED` with its reason.
#[derive(Default, Clone)]
pub struct TriggerWiring {
    pub listeners: Vec<ordo_exec::triggers::Listener>,
    pub clock: Vec<ordo_exec::triggers::ClockRoutine>,
    pub handler: Option<Arc<dyn crate::supervisor::RunHandler>>,
    /// The tenants the loop walks. `None` is the single space this process
    /// booted with, on the control database, which is what a test that
    /// supplies its own listeners gets.
    pub tenants: Option<Arc<crate::routines::Tenants>>,
}

/// Steps 2–14 of the assembly, over the config struct.
///
/// Four orderings are load-bearing, and a hurried entrypoint reorders all
/// four:
///
/// - **Every environment refusal precedes every dial** — that is
///   [`RuntimeConfig::from_env`]'s job, and it has already run.
/// - **The gate precedes the surface.** Enforced by the type system:
///   `PlatformState::new` takes an `Arc<Deployment>`, `Deployment::boot`
///   takes a `Verified`, and `Verified`'s only constructors run the gate.
///   This function's contribution is to *not* reach for
///   `Deployment::pinned` when the gate is red.
/// - **The drain starts before the door.** The door writes denials into a
///   bounded in-memory recorder that is lost on restart; starting the door
///   first opens a window whose only denials are the ones that get dropped.
/// - **The supervisor starts before the door** (R3), so a write that lands
///   before `drain_listener` has a cursor does not make the first drain
///   walk the whole log.
pub async fn assemble(cfg: RuntimeConfig) -> Result<Runtime, BootRefusal> {
    assemble_program(cfg, ProgramSource::Registry).await
}

/// `ordo dev`'s entry point (`plan-dev-loop.md` D3) — the same assembly,
/// gate included, over a fixture that did not come from the artifact
/// registry and never will (§4.3). Every property [`assemble`] gives a
/// server, this gives `ordo dev`: the gate runs, `Deployment::swap` is the
/// only way in, and a candidate that seals under a different `KeyProvider`
/// is refused exactly as it would be at boot. See
/// [`crate::deploy::verify_embedded`] for the one place this path departs
/// from the registry's — which DDL it gates against, and why.
pub async fn assemble_embedded(
    cfg: RuntimeConfig,
    fixture: ordo_conformance::Fixture,
) -> Result<Runtime, BootRefusal> {
    assemble_program(cfg, ProgramSource::Embedded(Box::new(fixture))).await
}

/// Where the boot's program comes from — the one fork [`assemble`] and
/// [`assemble_embedded`] share everything around. Boxed: `Registry` carries
/// nothing and a `Fixture` is hundreds of bytes, and every `RuntimeConfig`
/// pairs with exactly one of these, once, at boot — not a hot path worth an
/// unboxed variant for.
enum ProgramSource {
    /// Every server: whatever `ordo apply` last activated.
    Registry,
    /// `ordo dev`, a single-program self-host, the deployment tests: a
    /// fixture that did not come from the registry, gated exactly the same
    /// way (`Verified::from_gate`'s own doc names this caller).
    Embedded(Box<ordo_conformance::Fixture>),
}

async fn assemble_program(
    mut cfg: RuntimeConfig,
    program: ProgramSource,
) -> Result<Runtime, BootRefusal> {
    // Before any engine is built: every door this process ever builds reads
    // the same presence policy.
    crate::presence::install(cfg.presence.clone());
    // Whose two-step claims count as presence (`plan-auth-federated.md`
    // §8.2): every OIDC connection at its setting, `off` unless named. The
    // tenants' choices are read into it once their table exists.
    crate::presence::install_upstream(Arc::new(crate::presence::acceptance(
        cfg.auth.iter().flat_map(|a| a.oidc.iter()),
        &cfg.space.0,
    )));
    crate::pause::install(cfg.pausing.clone());
    crate::delegable::install(cfg.delegable.clone());
    crate::access_template::install(cfg.access_template.clone());
    let metrics = Arc::new(Metrics::new());

    // 3 — the pool. Dials every connection, so a bad config fails here
    // rather than on the first request.
    let pool = Pool::connect(cfg.pg.clone())
        .await
        .map_err(|e| connect_refusal(&e.to_string(), &cfg.pg.host, cfg.pg.port))?;
    pool.ensure_platform_tables()
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;

    // **One connection for the three stores that each used to dial their
    // own.** The keystore, the credential store and the principal
    // directory are each serialised through a single job loop, so between
    // them they can never use more than one connection at a time. Three
    // was three sockets held for one consumer's worth of work, and a
    // deployment now holds two fewer.
    //
    // **Separate from `pool`, and that part is a rule rather than a
    // preference.** A request checks out a `TxConn` and, while still
    // holding it, reaches the keystore to unwrap a data key
    // (`ordo_exec::sealed`). A store drawing from the request pool would
    // be a task holding one permit and waiting for a second from the same
    // pool of N; at N concurrent requests every permit is held by a waiter
    // and nothing finishes. `acquire_timeout` turns that into `Saturated`
    // for everyone, which is load-triggered and gets worse as the pool
    // grows.
    //
    // **More than one connection since 2026-10-24.** It was one, to save
    // sockets, when it served three stores. It now serves about a dozen,
    // and one commit that waited 2 s on its WAL fsync turned every other
    // sign-in store away as `server_busy` (board task
    // `store-pool-held-blocks-signin`). Each store still runs one statement
    // at a time through its own job loop, and each call is one autocommitted
    // statement whose caller waits for the commit, so a second connection
    // changes no store's ordering. See [`store_pool_config`] for the size
    // and the wait.
    let store_pool = Pool::connect(store_pool_config(
        &|k: &str| std::env::var(k).ok(),
        &cfg.pg,
    )?)
    .await
    .map_err(|e| connect_refusal(&e.to_string(), &cfg.pg.host, cfg.pg.port))?;
    // Every sign-in depends on this pool. When a store is turned
    // away as busy, the log names the store holding it, for how long, and
    // whether it is dialling or holding it (board task
    // `store-pool-held-blocks-signin`: in CI it stayed held for about 2
    // minutes and nothing said by whom).
    //
    // Once per incident (at most once a minute) it also logs Postgres's own
    // view: this deployment's backends, what each waits on and who blocks
    // it, and the locks waited for, taken through a connection of its own
    // on a thread of its own (`ordo_exec::incident`). The holder line says
    // which store; this says whether its statement waits on a lock or was
    // answered and not read. `ORDO_PG_BUSY_SNAPSHOT=off` turns it off.
    let snapshot = ordo_exec::incident::snapshot_enabled(
        std::env::var(ordo_exec::incident::SNAPSHOT_ENV)
            .ok()
            .as_deref(),
    )
    .unwrap_or(true);
    let gate = Arc::new(ordo_exec::incident::Every::new(
        ordo_exec::incident::SNAPSHOT_EVERY,
    ));
    let control = store_pool.config();
    let app = ordo_exec::incident::application_name("store pool");
    store_pool.track_holders(
        "store pool",
        Arc::new(move |pool, refused, holders| {
            eprintln!("{}", ordo_exec::holders::describe(pool, refused, holders));
            if snapshot && gate.due() {
                ordo_exec::incident::spawn_snapshot(control.clone(), Some(app.clone()), |block| {
                    eprintln!("{block}")
                });
            }
        }),
    );
    // 17n M2: `$quotas`, always — an operator may run `ordo quota set`
    // before ever setting `ORDO_USAGE_LEDGER`, and a table that only
    // appeared once metering was on would make that ordinary sequencing
    // fail with a missing-relation error instead of a clear refusal.
    crate::quotas::ensure_table(&pool)
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;
    // 17aa piece 3: `$spaces`, always, for `$quotas`' reason one tier over.
    // It is created here rather than by the first `std.space.create`
    // because a table created by the act it records is a table that does
    // not exist when the act fails, and the first thing an operator does
    // after a failed create is go looking for the row.
    crate::tenants::ensure_table(&pool)
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;
    let pool = pool.observing(metrics.clone());

    // 5 — custody.
    //
    // The custody table first, for the reason the registry's `ensure_tables`
    // moved here on 2026-09-19 and in the same place: `PgKeyStore::open`
    // reads `$keycustody`, a database nobody has run `ordo keys init`
    // against does not have it, and that is the database a first deployment
    // is pointed at. Creating the table is not minting custody: an empty
    // `$keycustody` grants nothing, and `LocalKeyProvider::release` still
    // refuses a space with no SMK in it, so the rule that a tenant is
    // never provisioned as a boot side effect is untouched by this line.
    ordo_exec::keystore::ensure_tables(&pool)
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;
    // The security recorder is made here, ahead of the custody that records
    // each release into it, the auth state that writes to it, and the drain
    // and door that read it: one recorder, so a key release, a refusal at
    // `/auth/complete` and a denial at the door are one stream.
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    // 25ab (`key-release-unrecorded`): every holder of the keys below takes
    // a copy of this one metered handle, so each release is counted on
    // `ordo_key_release_total` and recorded as `KeyReleased` in the stream
    // of the space it was released for. The release-breadth alert reads
    // that counter. Wrapped once, here, and nowhere else in the runtime.
    let keys = meter_custody(
        build_custody(&cfg, store_pool.clone())?,
        &metrics,
        &recorder,
    );

    // What each tenant requires of a session's second step, held once and
    // shared by the door, the sign-in gate and the admission op, so a level an
    // owner sets is the level the next request meets.
    let second_levels = Arc::new(
        crate::second_factors::SecondStepLevels::new(cfg.second_factor.mode)
            .with_upstream(crate::presence::upstream()),
    );

    // The tenant credential provider (18d T3, 18e U4), built once here and
    // reused by every door rebuild and by the resolver. `$deployment`
    // answers with the password this deployment already holds, so a
    // deployment that configured no credentials directory still reaches
    // every tenant database on its own cluster. See [`tenant_credentials`].
    let credentials = tenant_credentials(cfg.credentials.clone(), cfg.pg.password.clone());
    // Read from the concrete type, before it is erased above: names only,
    // for `/health/ready` (§9.7 C-M3). An empty list for a deployment that
    // configured no provider, which is every deployment before this field
    // existed and remains a legitimate answer.
    let credential_labels: Vec<String> = cfg
        .credentials
        .as_ref()
        .map(|dir| dir.labels())
        .unwrap_or_default();

    // The platform ops this node serves, decided by configuration before
    // the load so the boot can vet the program against them.
    let platform = platform_specs(
        cfg.exit_bundles.is_some(),
        cfg.import_bundles.is_some(),
        cfg.checkpoint.is_some(),
        buckets_wired(&cfg),
    );
    if let Some(dir) = &cfg.exit_bundles {
        std::fs::create_dir_all(dir).map_err(|e| BootRefusal::Invalid {
            var: "ORDO_EXIT_BUNDLE_DIR",
            detail: format!("`{}` could not be created: {e}", dir.display()),
        })?;
    }
    // **Checked, not created**, which is the opposite of the line above and
    // for the reason that separates an output from an input: creating an
    // export directory makes a deployment that can be left, and creating
    // an import directory makes an empty one whose first `bundle_unknown`
    // an operator would have to debug from the outside. A staging root the
    // operator has not put there is a configuration error, and the boot is
    // where a configuration error is cheapest to say.
    if let Some(dir) = &cfg.import_bundles {
        if !dir.is_dir() {
            return Err(BootRefusal::Invalid {
                var: "ORDO_IMPORT_BUNDLE_DIR",
                detail: format!(
                    "`{}` is not a directory; a bundle is staged there before it is \
                     imported, so this deployment would register an import op with \
                     nowhere to read from",
                    dir.display()
                ),
            });
        }
    }

    // 6/7 — load the program, and assert it seals through the provider
    // this binary built. R2a's `verify_candidate`/`verify_embedded` assert
    // the same thing; two independent mechanisms agreeing is the
    // arrangement the sealed-handle scoping already uses.
    // The registry's own tables, before it is read. A database nobody has
    // run `ordo apply` against has none, and the binary must boot there
    // into the no-program state rather than die on a missing relation:
    // that is the database the first deploy is made to.
    ordo_exec::registry::ensure_tables(&pool)
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;
    let deployment = match program {
        ProgramSource::Registry => {
            match deploy::load_active(&pool, &cfg.pg, keys.clone(), cfg.space.clone()).await {
                Ok(verified) => {
                    if !verified.custody_is(&keys) {
                        return Err(BootRefusal::CustodyMismatch);
                    }
                    // Refuses a program that compiled a reserved
                    // `std.space.*` name, rather than letting either side
                    // shadow the other.
                    Deployment::boot_with_platform_ops(verified, platform)
                        .map_err(BootRefusal::Load)?
                }
                // Not a boot failure. A database nobody has run `ordo apply`
                // against yet is a deployment waiting for one, and refusing
                // here would make the ordinary first deploy impossible.
                Err(LoadRefusal::NoActiveVersion) => Deployment::empty_with_platform_ops(
                    cfg.space.clone(),
                    pool.clone(),
                    keys.clone(),
                    platform,
                )
                .map_err(BootRefusal::Load)?,
                Err(other) => return Err(BootRefusal::Load(other)),
            }
        }
        ProgramSource::Embedded(fixture) => {
            // "dev-boot": distinct from every content-derived label
            // `ordo dev`'s own save loop mints afterward (`dev.rs`'s
            // `dev_version` hex-encodes a digest, and "boot" is not valid
            // hex), so the first save-triggered swap is never mistaken for
            // `AlreadyServing` against this one.
            let verified = deploy::verify_embedded(
                *fixture,
                &pool,
                &cfg.pg,
                keys.clone(),
                cfg.space.clone(),
                "dev-boot",
            )
            .await
            .map_err(BootRefusal::Load)?;
            // Custody is asserted a second time inside `verify_embedded`
            // itself (by identity, not by type name) — this arm has no
            // separate check to duplicate, unlike the registry arm above,
            // because there is no path here that could hand back a
            // candidate `verify_embedded` did not already build `keys`
            // into.
            Deployment::boot_with_platform_ops(verified, platform).map_err(BootRefusal::Load)?
        }
    };

    // **The resolver, built here and built once** (18d T2).
    //
    // It used to be built at the far end of this function, after the
    // replay cache, the security drain and the door had each constructed a
    // `SinglePool` of their own. Four resolvers over one deployment is
    // four answers to "which database are this space's rows in", and they
    // agree only for as long as every space is on the default pool. The
    // moment one is attached elsewhere, whichever surface still holds its
    // own resolver reads the wrong database and does so silently, because
    // a pool that answers is indistinguishable from a pool that answers
    // correctly.
    //
    // So it moves to the top, directly behind the deployment it resolves
    // for, and everything below is handed `spaces.pools()`. The ordering
    // is forced rather than chosen: the replay cache is constructed well
    // before the door, so the resolver has to exist before the cache, and
    // the only thing it needs is the deployment and a pool, both of which
    // are in hand here.
    //
    // Built with the witness rather than through `Spaces::new`, so the
    // handle the boot reconciliation uses is the handle every `resolve`
    // uses: one record, read the same way at both sites.
    let witness = ordo_exec::witness::Witness::new(pool.clone());
    // **Every space is resolved through `$spacedb`** (ruled 2026-09-25,
    // 2026-10-06). A tenant has a row there naming its own database; a
    // space with no row, which is the deployment's own system space,
    // resolves to the default, the control database this deployment
    // booted with.
    //
    // The control pool and the default pool are the same handle here and
    // are passed separately because they are not the same *question*: the
    // control pool is where `$spacedb` is read from, and the default is
    // where a space with no row is served from. A deployment that later
    // moves its control plane changes one without changing the other.
    let resolver: Arc<dyn ordo_exec::route::PoolResolver> =
        Arc::new(ordo_exec::route::AttachedPools::new(
            pool.clone(),
            deployment.current().exec.pool.clone(),
            cfg.pg.clone(),
            credentials.clone(),
        ));
    let spaces = crate::spaces::Spaces::routed(deployment.clone(), resolver, witness.clone());
    spaces.set_pause_answers(cfg.pausing.answers().clone());

    // 17v: the system space, after the program and before the door.
    //
    // **After the program**, because the founding member row goes into
    // `members`, which a program's DDL creates: on a database an `ordo
    // apply` has already reached, this boot writes the row; on an empty one
    // it mints custody and says so. **Before the door**, because the access
    // engine is built from `members` a few dozen lines below, so a system
    // space created by this boot is a space *this* boot serves rather than
    // the next one.
    let system_space = crate::provision::ensure_system_space(&pool, keys.as_ref())
        .await
        .map_err(BootRefusal::SystemSpace)?;

    // The one refusal that follows the dial, and it is forced: whether a
    // blob store is *needed* is a property of the deployed program, which
    // lives in the database. `blob_from_env` already refused a malformed
    // `ORDO_BLOB_STORE` before the dial, so a typo is still fast; what is
    // decided here is necessity.
    if let Some(field) = blob_needed(&deployment.current().fixture.schema) {
        if cfg.blob.is_none() {
            return Err(BootRefusal::Missing {
                var: "ORDO_BLOB_STORE",
                why: format!(
                    "this program declares `{field}` as a stored blob, so the door has a \
                     file route with nowhere to put the bytes (s3 | fs:<path>)"
                ),
            });
        }
    }

    // The tables the door's own state lives in. Idempotent, and the
    // deployment's rather than the operator's: these carry no program.
    let delegations = Arc::new(crate::delegation::PgDelegationRegistry::new(pool.clone()));
    delegations
        .ensure_table()
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;
    // T2: the one resolver, not a `SinglePool` of its own. The idempotency
    // record for a call in an attached tenant belongs in that tenant's
    // database beside the rows the call wrote, and a cache resolving
    // through its own pool would put it in the control database instead.
    // A replayed key would then find no record and re-execute the write.
    let replay = Arc::new(crate::replay::PgReplayCache::routed(
        spaces.pools().clone(),
        keys.clone(),
    ));
    replay
        .ensure_table()
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;

    // ── authentication: the credential store and the session deny list ──
    //
    // Built before the door, because the door takes the deny list. Both
    // tables are the deployment's rather than the operator's, like
    // `$delegation` above: an empty credential table is a deployment where
    // nobody has signed up yet, which is the ordinary first minute of one.
    //
    // **No fallback.** If the operator asked for authentication and either
    // store cannot be opened, the boot refuses. Falling back to the
    // in-memory pair would be a deployment that authenticates people until
    // it restarts and then silently has no accounts, which is the exact
    // failure this increment exists to remove.
    // P0: the deployment owners the environment names, after the system
    // space and before the door reads `members`, so this boot serves them.
    // A failure is a refusal: a deployment that was told who owns it and
    // cannot write that down must not come up ownerless and look healthy.
    report_deployment_owners(
        crate::provision::ensure_deployment_owners(
            &pool,
            keys.as_ref(),
            &cfg.space.0,
            &cfg.deployment_owners,
            Some(recorder.as_ref()),
            now_secs(),
        )
        .await
        .map_err(BootRefusal::SystemSpace)?,
        &cfg.space.0,
    );
    // The human check leaves the same way: it holds a secret.
    let mut human_check = cfg.auth.as_mut().and_then(|a| a.human_check.take());
    // The connections leave the configuration here, because each holds a
    // client secret that is not `Clone` and goes into exactly one provider.
    let mut connections: std::collections::BTreeMap<
        String,
        ordo_identity::oauth2::OAuth2Connection,
    > = cfg
        .auth
        .as_mut()
        .map(|a| std::mem::take(&mut a.oauth2))
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c.name.clone(), c))
        .collect();
    let mut oidc_connections: std::collections::BTreeMap<
        String,
        ordo_identity::oidc::OidcConnection,
    > = cfg
        .auth
        .as_mut()
        .map(|a| std::mem::take(&mut a.oidc))
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c.name.clone(), c))
        .collect();
    let auth = match &cfg.auth {
        None => None,
        Some(a) => {
            crate::credentials::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::sessions::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::principal_directory::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::passkeys::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::ceremonies::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::recovery::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::recovery_codes::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::second_factor_store::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::federation::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::contact_email::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            if a.contact_codes.is_some() {
                crate::mail::ensure_table(&pool)
                    .await
                    .map_err(|e| BootRefusal::Database(e.to_string()))?;
                crate::email_code::ensure_table(&pool)
                    .await
                    .map_err(|e| BootRefusal::Database(e.to_string()))?;
            }
            // A connection keeps its endpoints for life under its name
            // (§3.5): a boot that finds them changed refuses.
            let fingerprints = connections
                .values()
                .map(|c| (c.name.clone(), "oauth2", c.fingerprint()))
                .chain(
                    oidc_connections
                        .values()
                        .map(|c| (c.name.clone(), "oidc", c.fingerprint())),
                );
            for (name, kind, fingerprint) in fingerprints {
                crate::federation::ensure_fingerprint(&pool, &name, kind, &fingerprint, now_secs())
                    .await
                    .map_err(|detail| BootRefusal::Invalid {
                        var: "ORDO_AUTH_METHODS",
                        detail,
                    })?;
            }
            let ceremonies: Arc<dyn ordo_identity::CeremonyStore> = Arc::new(
                crate::ceremonies::PgCeremonyStore::open_on(store_pool.clone())
                    .map_err(|e| BootRefusal::Database(e.to_string()))?,
            );
            let store: Arc<dyn ordo_identity::CredentialStore> = Arc::new(
                crate::credentials::PgCredentialStore::open_on(store_pool.clone())
                    .map_err(|e| BootRefusal::Database(e.to_string()))?,
            );
            let pg_revocations = Arc::new(
                crate::sessions::PgSessionRevocations::open(pool.clone())
                    .await
                    .map_err(|e| BootRefusal::Database(e.to_string()))?,
            );
            // Renewals end with the session they belong to: the deny list
            // every caller holds is wrapped once, here ([`crate::renewal`]).
            crate::renewal::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            crate::session_policy::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            // The password slowdown: its layers and the count every node
            // shares ([`crate::guess_limits`]).
            crate::guess_limits::ensure_table(&pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            let guess_layers = crate::guess_limits::Deployment::from_env()
                .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
            let renewals: Arc<dyn crate::renewal::RenewalStore> =
                Arc::new(crate::renewal::PgRenewals::new(pool.clone()));
            let revocations: Arc<dyn ordo_identity::session::SessionRevocations> = Arc::new(
                crate::renewal::RenewingRevocations::new(pg_revocations.clone(), renewals.clone()),
            );
            let directory_store: Arc<dyn ordo_identity::DirectoryStore> = Arc::new(
                crate::principal_directory::PgDirectoryStore::open_on(store_pool.clone())
                    .map_err(|e| BootRefusal::Database(e.to_string()))?,
            );
            let mut registry = ordo_identity::ProviderRegistry::new();
            let mut passkey: Option<Arc<ordo_identity::PasskeyProvider>> = None;
            let mut oauth2: Vec<Arc<ordo_identity::oauth2::OAuth2Provider>> = Vec::new();
            let mut oidc: Vec<Arc<ordo_identity::oidc::OidcProvider>> = Vec::new();
            let mut email: Option<Arc<crate::email_code::EmailCodeProvider>> = None;
            for method in &a.methods {
                // `auth_from_env` already refused anything but these two, so
                // the refusal lives where an operator meets it rather than
                // here.
                let provider: Box<dyn ordo_identity::AuthProvider> = match method.as_str() {
                    "password" => Box::new(
                        ordo_identity::LocalPasswordProvider::with_store(
                            // The real profile, never `weak_for_tests`: this
                            // is the one construction a deployment reaches.
                            ordo_identity::Argon2idHasher::new(),
                            store.clone(),
                        )
                        .with_pending_max(a.password_pending_max as usize),
                    ),
                    "passkey" => {
                        let rp = a
                            .relying_party
                            .clone()
                            .expect("auth_from_env builds a relying party for passkey");
                        let keys: Arc<dyn ordo_identity::PasskeyStore> = Arc::new(
                            crate::passkeys::PgPasskeyStore::open_on(store_pool.clone())
                                .map_err(|e| BootRefusal::Database(e.to_string()))?,
                        );
                        let p = Arc::new(ordo_identity::PasskeyProvider::with_stores(
                            rp,
                            keys,
                            ceremonies.clone(),
                        ));
                        passkey = Some(p.clone());
                        Box::new(p)
                    }
                    crate::email_code::METHOD => {
                        let policy = a
                            .email
                            .clone()
                            .expect("auth_from_env reads the email policy for `email`");
                        let codes: Arc<dyn crate::email_code::EmailCodeStore> = Arc::new(
                            crate::email_code::PgEmailCodeStore::open_on(store_pool.clone())
                                .map_err(|e| BootRefusal::Database(e.0.to_string()))?,
                        );
                        let queue: Arc<dyn crate::mail::MailQueue> = Arc::new(
                            crate::mail::PgMailQueue::open_on(store_pool.clone())
                                .map_err(|e| BootRefusal::Database(e.to_string()))?,
                        );
                        let p = Arc::new(
                            crate::email_code::EmailCodeProvider::new(
                                &cfg.token_secret,
                                policy,
                                codes,
                                queue,
                                directory_store.clone(),
                                if a.auto_onboard {
                                    ordo_identity::Provisioning::AutoOnboard
                                } else {
                                    ordo_identity::Provisioning::InviteOnly
                                },
                            )
                            .with_recorder(recorder.clone()),
                        );
                        email = Some(p.clone());
                        Box::new(crate::email_code::Registered(p))
                    }
                    name if oidc_connections.contains_key(name) => {
                        let conn = oidc_connections
                            .remove(name)
                            .unwrap_or_else(|| unreachable!("checked just above"));
                        let p = Arc::new(
                            ordo_identity::oidc::OidcProvider::new(
                                conn,
                                ceremonies.clone(),
                                crate::oidc::EgressFactory::deployment(
                                    a.egress_roots.clone(),
                                    a.egress_deny.clone(),
                                ),
                            )
                            .map_err(|e| BootRefusal::Invalid {
                                var: "ORDO_AUTH_METHODS",
                                detail: format!("connection `{name}`: {e}"),
                            })?,
                        );
                        oidc.push(p.clone());
                        Box::new(p)
                    }
                    name => {
                        let conn = connections
                            .remove(name)
                            .unwrap_or_else(|| unreachable!("auth_from_env refused `{name}`"));
                        let transport = crate::federation::deployment_transport(
                            &conn,
                            a.egress_roots.clone(),
                            a.egress_deny.clone(),
                        );
                        let p = Arc::new(
                            ordo_identity::oauth2::OAuth2Provider::new(
                                conn,
                                ceremonies.clone(),
                                transport,
                            )
                            .map_err(|e| BootRefusal::Invalid {
                                var: "ORDO_AUTH_METHODS",
                                detail: format!("connection `{name}`: {e}"),
                            })?,
                        );
                        oauth2.push(p.clone());
                        Box::new(p)
                    }
                };
                registry
                    .register(method, provider)
                    .map_err(|e| BootRefusal::Invalid {
                        var: "ORDO_AUTH_METHODS",
                        detail: e.to_string(),
                    })?;
            }
            // A guest's `resume` is always served: it answers only a secret
            // the directory already holds, so with guests never switched on
            // it answers nobody. A new guest is the stored setting's
            // (`auth::guest`).
            registry
                .register(
                    crate::auth::guest::METHOD,
                    Box::new(crate::auth::guest::GuestProvider::new(
                        directory_store.clone(),
                    )),
                )
                .map_err(|e| BootRefusal::Invalid {
                    var: "ORDO_AUTH_METHODS",
                    detail: e.to_string(),
                })?;
            let directory = ordo_identity::directory::Directory::with_store(
                if a.auto_onboard {
                    ordo_identity::Provisioning::AutoOnboard
                } else {
                    ordo_identity::Provisioning::InviteOnly
                },
                directory_store.clone(),
            );
            let mut state = crate::auth::AuthState::new(
                registry,
                directory,
                // **The same key the door verifies with**, derived from the
                // same secret rather than read from the environment a
                // second time. A deployment that mints under one key and
                // verifies under another authenticates nobody and does it
                // silently.
                ordo_identity::TokenKey::from_secret(&cfg.token_secret),
            )
            .with_revocations(revocations.clone())
            .with_recorder(recorder.clone())
            .with_ceremonies(ceremonies)
            .with_session_store(pg_revocations)
            .with_session_lifetime(a.session_secs, a.session_max_secs)
            .with_remember(crate::renewal::Remember {
                store: renewals,
                config: a.remember,
                app_origins: cfg.cors.clone(),
            })
            .with_policy(Arc::new(
                crate::session_policy::SessionPolicies::on_pool(a.session_layers, pool.clone())
                    .with_home(cfg.space.0.clone())
                    .with_stream(recorder.clone()),
            ))
            .with_guess_limits(Arc::new(
                crate::guess_limits::GuessLimits::on_pool(guess_layers, pool.clone())
                    .with_home(cfg.space.0.clone())
                    .with_stream(recorder.clone()),
            ))
            .with_handoff(a.returns.clone(), a.handoff_secs)
            .with_linking(a.linking);
            state.methods_max_age = a.methods_max_age;
            let mut state = match passkey {
                Some(p) => state.with_passkey(p),
                None => state,
            };
            for p in oauth2 {
                state = state.with_oauth2(p);
            }
            for p in oidc {
                state = state.with_oidc(p);
            }
            if let Some(p) = email {
                state = state.with_email(p);
            }
            // Guest sign-in: the environment seeds the stored setting, and
            // the boot says so when guests are on with no human check.
            let guest_store = crate::auth::guest::GuestStore::new(pool.clone());
            for note in guest_store
                .boot(&a.guest, now_secs())
                .await
                .map_err(|detail| BootRefusal::Invalid {
                    var: crate::auth::guest::SETTING,
                    detail: detail.to_string(),
                })?
            {
                eprintln!("  {note}");
            }
            if let Ok(mode) = guest_store.get().await {
                if let Some(line) = crate::auth::guest::unguarded_line(mode, human_check.is_some())
                {
                    eprintln!("  {line}");
                }
            }
            let state = state.with_guests(crate::auth::guest::Guests::stored(guest_store));
            let state = match human_check.take() {
                Some(check) => state.with_human_check(check),
                None => state,
            };
            let state = state.with_identity_attrs(Arc::new(
                crate::federation::PgIdentityAttrs::open_on(store_pool.clone())
                    .map_err(|e| BootRefusal::Database(e.0.to_string()))?,
            ));
            // A person's own verified address (owner ruling 2026-10-22).
            // Its codes come from the sign-in provider when `email` is a
            // method, else from one made for confirming addresses only,
            // which the registry never holds.
            let mut contact = crate::contact_email::ContactEmails::on_pool(
                a.email_required.clone(),
                pool.clone(),
            )
            .with_home(cfg.space.0.clone())
            .with_change_touch(a.email_change_touch)
            .with_stream(recorder.clone());
            if let Some(policy) = a.contact_codes.clone() {
                let queue: Arc<dyn crate::mail::MailQueue> = Arc::new(
                    crate::mail::PgMailQueue::open_on(store_pool.clone())
                        .map_err(|e| BootRefusal::Database(e.to_string()))?,
                );
                let codes = match state.email.clone() {
                    Some(p) => p,
                    None => {
                        let store: Arc<dyn crate::email_code::EmailCodeStore> = Arc::new(
                            crate::email_code::PgEmailCodeStore::open_on(store_pool.clone())
                                .map_err(|e| BootRefusal::Database(e.0.to_string()))?,
                        );
                        Arc::new(
                            crate::email_code::EmailCodeProvider::new(
                                &cfg.token_secret,
                                policy,
                                store,
                                queue.clone(),
                                directory_store.clone(),
                                ordo_identity::Provisioning::InviteOnly,
                            )
                            .with_recorder(recorder.clone()),
                        )
                    }
                };
                contact = contact.with_mail(codes, queue);
            }
            if !a.email_required.is_off() {
                eprintln!(
                    "  {}={}: a signed-in person needs a verified email address for those ops",
                    crate::contact_email::REQUIRED_ENV,
                    a.email_required.word()
                );
            }
            let state = Arc::new(state);
            state.wire_contact(Arc::new(contact));
            // G14: console keys, in the control database. A challenge is
            // stateless, under a MAC key every replica derives from the
            // token secret.
            let console_store = crate::console_keys::ConsoleKeyStore::pg(pool.clone());
            console_store
                .ensure_tables()
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            let mut keys = crate::console_keys::ConsoleKeys::new(
                console_store,
                crate::console_keys::ConsoleKeys::challenge_key_from(&cfg.token_secret),
                a.console_session_secs,
                &cfg.space.0,
            );
            // With the identity service the console calls the door as the
            // signed-in person; a console key left here must not sign in
            // (`plan-identity-service.md` 6, G14).
            if crate::identity_keys::IdentitySettings::from_env()
                .ok()
                .flatten()
                .is_some()
            {
                keys = keys.closed_by(crate::identity_keys::URL_ENV);
            }
            state.wire_console_keys(Arc::new(keys));
            Some((state, revocations))
        }
    };
    let revocations = auth.as_ref().map(|(_, r)| r.clone());

    // 11 — the security drain, BEFORE the door.
    //
    // Retention is handed to the supervisor's `secevent.retain` worker
    // rather than run from the drain's own loop, so it is leader-only,
    // counted and quarantinable like every other cadence.
    // T2: the one resolver again. A security event carries a space, and
    // the space-scoped half of `$secevents` lives in that space's own
    // database; a drain resolving through its own pool would write a
    // tenant's events into the control database, where the tenant's own
    // retention pass would never see them.
    //
    // The checkpoint key first (`plan-identity-service.md` section 7): the
    // set one, or the one made at first boot and sealed in `$securitykey`.
    // No key turns the feature off and says why; it never refuses the boot.
    let checkpoint_signer = match crate::security_key::resolve(
        ordo_secevent::security_signer_from_env(),
        &crate::security_key::SecurityKeyStore::pg(pool.clone()),
        keys.as_ref(),
        now_secs(),
    )
    .await
    {
        Ok(key) => {
            eprintln!(
                "  security checkpoint key ({}); verify with {}",
                key.source.word(),
                key.published()
            );
            cfg.security_checkpoint = true;
            crate::secdrain::CheckpointSigner::Resolved(key.signer, key.source.word())
        }
        Err(e) => {
            eprintln!("  security checkpoints are off: {e}");
            cfg.security_checkpoint = false;
            crate::secdrain::CheckpointSigner::Unavailable(e.to_string())
        }
    };
    let secdrain = SecurityDrain::start_routed(
        recorder.clone(),
        pool.clone(),
        spaces.pools().clone(),
        metrics.clone(),
        crate::secdrain::DrainConfig {
            supervised_retention: true,
            checkpoint_signer,
            ..crate::secdrain::DrainConfig::default()
        },
    )
    .await
    .map_err(|e| BootRefusal::Database(e.to_string()))?;

    // 9/10 — the access engine from the POOL, never the `psql` fork.
    let live = deployment.current();
    let (door, unread) = door_for(
        &live.fixture,
        &pool,
        &cfg.token_secret,
        &metrics,
        recorder,
        delegations.clone(),
        revocations.clone(),
        &credentials,
    )
    .await?;
    let access = Arc::new(SwappableAccess::new(door));
    access.wire_second_step(second_levels.clone());
    access.wire_console_home(&cfg.space.0);
    if let Some(c) = auth.as_ref().and_then(|(a, _)| a.contact().cloned()) {
        access.wire_contact(c);
    }
    access.note_unread(unread);
    let grace = crate::directory::stale_grace_from(
        std::env::var(crate::directory::STALE_GRACE_SETTING)
            .ok()
            .as_deref(),
    )
    .map_err(|detail| BootRefusal::Invalid {
        var: crate::directory::STALE_GRACE_SETTING,
        detail,
    })?;
    access.set_stale_grace(grace).await;

    // Step 3 of `plan-identity-service.md`: the identity service's keys,
    // from `$identitykeys` first so a restart with the service down still
    // verifies, then fetched. Unset, the door accepts no deployment token.
    match crate::identity_keys::IdentitySettings::from_env()
        .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?
    {
        None => eprintln!(
            "  {} is unset: the door accepts no deployment token",
            crate::identity_keys::URL_ENV
        ),
        Some(settings) => {
            let store = crate::identity_keys::IdentityKeyStore::pg(pool.clone());
            store
                .ensure_table()
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            let shown = settings.url.display();
            let keys = Arc::new(crate::identity_keys::IdentityKeys::new(
                settings,
                store,
                Arc::new(now_secs),
            ));
            match keys.boot().await {
                Ok(()) => eprintln!("  identity service at {shown}: {} key(s)", keys.held_keys()),
                Err(e) => eprintln!(
                    "  identity service at {shown}: {e}; verifying with {} stored key(s) \
                     and fetching again on schedule",
                    keys.held_keys()
                ),
            }
            keys.spawn_refresher();
            access.wire_identity_keys(keys);
            if let Some(note) =
                crate::provision::ignored_deployment_owners_note(&|k: &str| std::env::var(k).ok())
            {
                eprintln!("  {note}");
            }
        }
    }

    // Cross-node invalidation, so `?live=1` means something with more than
    // one replica.
    //
    // **A listener per database, through the same resolver everything else
    // resolves through.** `?live=1` is delivered by Postgres `NOTIFY`, and
    // a notification reaches only sessions connected to the database it was
    // sent from (`invalidation.rs`'s own note). A bus listening on the
    // control database alone would deliver nothing at all for a tenant, and
    // would do it silently: the subscription succeeds, the write succeeds,
    // and the update never arrives.
    let pg_bus = Arc::new(
        crate::invalidation::PgInvalidationBus::connect_routed(
            &live.fixture,
            spaces.pools().clone(),
            Some(metrics.clone()),
        )
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?,
    );
    // Kept as the concrete bus as well: a tenant created after boot lives in
    // a database this bus was not listening to, and the two places that
    // learn of one (the members notification and the access refresh tick)
    // call `refresh` on it.
    let bus: Arc<dyn ordo_api::InvalidationBus> = pg_bus.clone();

    // Layer-1 attestation, when there is a key to sign with. `NoAttestation`
    // is the deliberate way to stand up a door that cannot attest — not an
    // absence, a stated posture.
    let disabled = census(&cfg);
    let security_checkpoint = cfg.security_checkpoint;
    // U5: taken before the destructure below, because both are read where
    // the blob tier is built and `cfg` is gone by then. The credential
    // provider is resolved here rather than inside the match arm for the
    // same reason the boot's own refusals are read where `cfg` is still
    // whole: the refusal belongs to configuration, not to assembly.
    let blob_routing = cfg.blob_routing;
    let bucket_credentials = match blob_routing {
        BlobRouting::Single => None,
        BlobRouting::Attached => Some(bucket_credentials_for_routing(&cfg)?),
    };
    // 18p U6, read here for `bucket_credentials`' reason: `cfg` is whole,
    // and the one condition that decides both the registration and the
    // wiring must be evaluated from the same value the registration above
    // used.
    let register_buckets = buckets_wired(&cfg);
    // The loopback exception's dev flag (D-S6). Read at the edge and passed
    // down, exactly as `HostAllowlist` is and for the same reason: a check
    // that reaches for a process global cannot be tested beside another
    // test that wants the other answer.
    let allow_plaintext_endpoint = matches!(
        std::env::var("ORDO_DEV_UNSAFE_BUCKET_ENDPOINT").as_deref(),
        Ok("1" | "true" | "yes")
    );
    let RuntimeConfig {
        pg,
        addr,
        shutdown_grace,
        space,
        custody: custody_choice,
        token_secret,
        page_key,
        checkpoint,
        apex_domain,
        cors,
        blob,
        exit_bundles,
        import_bundles,
        replica,
        usage_flush,
        usage_rollup,
        sync,
        ..
    } = cfg;
    // One signer, shared: the door's layer-1 attestation and the checkpoint
    // and attestation workers must sign under the same key, or a receiver
    // holding the published verifying key can check one and not the other.
    let signer_for_workers: Option<Arc<ordo_commandlog::Ed25519Signer>> = checkpoint.map(Arc::new);
    let attest: Arc<dyn ordo_api::attest::AttestationSource> = match &signer_for_workers {
        Some(signer) => Arc::new(crate::attest::PgAttestation::new(
            pool.clone(),
            Box::new(SharedSigner(signer.clone())),
        )),
        None => Arc::new(ordo_api::attest::NoAttestation),
    };

    // **Reconciliation at boot** (17aa). Before the drain starts and before
    // anything is served, each space this boot knows the name of has its
    // chain compared against the control database's record of the heads it
    // signed, which a restore of the space's own database cannot roll back.
    //
    // It **answers**; it does not refuse the process. The owner, 2026-09-21:
    // *"the system should not be dependent on the rest of the data designs,
    // deployment space, or tenants"*. A tenant whose chain moved backwards
    // is quarantined by `Spaces` and reported by `/health/ready`, and the
    // deployment keeps serving every other tenant and the control plane —
    // which is also what keeps the door up for `ordo restore --record`, the
    // command that explains the rewind. See [`reconcile_at_boot`] for what
    // this settles on its own and what it will not.
    let reconciled = reconcile_at_boot(&spaces, &space.0).await;

    // The blob tier, mounted when a store is configured.
    //
    // Held on the `Runtime` because the sweep (R3) and the exit ops both
    // need the same one — two resolvers over one bucket would be two
    // answers to "where are this space's bytes". The pipeline is shared
    // with the exit ops for the same reason.
    //
    // **One resolver, and the pipeline takes that one.** The door's write
    // and read path used to be handed the bare store while the sweep, the
    // re-seal pass and `erase_space` were handed the resolver, so the one
    // path that creates objects was the one path with no per-space seam.
    // It now takes `stores` itself, which is the same `Arc` the other
    // three ask, so there is exactly one answer to where a space's bytes
    // live and turning on an attached resolver reaches the door, the
    // export, the sweep and the erase together.
    //
    // **U5: which resolver, decided by `ORDO_BLOB_ROUTING` and by nothing
    // else.** `Single` is `SingleStore` over the one binding
    // `ORDO_BLOB_STORE` named, which is every deployment before U5 and
    // stays the default. `Attached` reads `$spacebucket`, so a space with
    // a row there resolves to its own bucket and every other space still
    // resolves to that same binding, which is what makes turning the
    // switch on a no-op until something is actually attached.
    //
    // The control pool and the default store are passed separately
    // because they are not the same question, exactly as T4 passes the
    // control pool and the default pool: one is where the attachment is
    // read, the other is where a space with no attachment is served.
    let (blob_label, blobs, stores, pipeline, buckets, blob_door) = match blob {
        None => (None, None, None, None, None, None),
        Some(BlobBinding { store, label, own }) => {
            let stores: Arc<dyn ordo_blob::StoreResolver> = match bucket_credentials.clone() {
                None => Arc::new(ordo_blob::SingleStore::new(store)),
                // The table is ensured here and only here, on the one path
                // that reads it. A deployment on `single` issues no
                // statement it did not issue before the switch existed,
                // which is half of what "behaves identically" has to mean;
                // `bucket_ops::attach` ensures it on its own path for the
                // same reason.
                Some(credentials) => {
                    ordo_exec::bucket::ensure_table(&pool)
                        .await
                        .map_err(|e| BootRefusal::Database(e.to_string()))?;
                    Arc::new(ordo_exec::bucket::AttachedStores::new(
                        pool.clone(),
                        store,
                        credentials,
                    ))
                }
            };
            let pipeline = Arc::new(ordo_blob::BlobPipeline::new(
                stores.clone(),
                keys.clone(),
                ordo_blob::BlobPolicy::DEFAULT,
            ));
            // **One door, two handles.** The HTTP blob route takes it as a
            // `BlobPort` and `std.space.import` takes it as a `BlobDoor`,
            // and they are the same `Arc` rather than two structures built
            // the same way: `PlatformBlobs`' own field doc says why a second
            // resolver would be a second answer to where a space's rows and
            // bytes are, and `plan-blob-chain.md` C1-C8 ruled there is one
            // upload path. Two doors would satisfy `upload_seam.rs`, which
            // counts call sites in the source, and still be the defect it
            // exists to catch.
            let door = Arc::new(crate::blob::PlatformBlobs {
                spaces: spaces.clone(),
                pipeline: (*pipeline).clone(),
            });
            let port: Arc<dyn ordo_api::BlobPort> = door.clone();
            // **18p U6: `std.space.attachBucket`'s state.** Built only when
            // `register_buckets` says the op is registered, which is the
            // `PlatformOps` invariant read from one place. The pieces are
            // each argued where they are declared on `BucketOps`; what is
            // decided *here* is that the resolver's credential provider and
            // the op's are the **same** `Arc`, so the material the probe
            // signs with is the material the next upload signs with.
            let buckets = match (register_buckets, bucket_credentials) {
                (true, Some(credentials)) => {
                    Some(Arc::new(crate::bucket_ops::BucketOps {
                        pool: pool.clone(),
                        pools: spaces.pools().clone(),
                        stream: Some(access.recorder.clone()),
                        credentials,
                        // One list, shared with the database tier (D-S6).
                        // The same constructor `tenant_ops::Routing` reads.
                        hosts: crate::provision::HostAllowlist::from_env(),
                        own_store: own,
                        allow_plaintext: allow_plaintext_endpoint,
                        probe: Arc::new(crate::bucket_ops::HeadProbe),
                        now: Arc::new(now_secs),
                    }))
                }
                // `register_buckets` already requires a credential
                // provider, so the second arm is unreachable from
                // `from_env`. It is a `None` rather than a refusal because
                // an embedder who hand-built a config with one and not the
                // other gets an unregistered op, which is the state
                // `PlatformOps` is built to express, rather than a boot
                // that dies over an optional feature.
                _ => None,
            };
            (
                Some(label),
                Some(ordo_api::blob::BlobState {
                    port,
                    limits: ordo_api::blob::BlobLimits::DEFAULT,
                }),
                Some(stores),
                Some(pipeline),
                buckets,
                Some(door as Arc<dyn ordo_exec::import::BlobDoor>),
            )
        }
    };

    // The platform ops' state, from the runtime's own pieces.
    let recovery_handle: crate::identity_ops::RecoveryHandle = Arc::default();
    // G3: an accepted share writes its acl rows through the same executor
    // the door dispatches program ops to, as the sharer, so the rows land in
    // the sharer's own chain under the sharer's guards.
    let acl: Arc<dyn crate::share_accept::AclRunner> = Arc::new(crate::share_accept::ExecutorAcl {
        executor: Arc::new(crate::FixtureExecutor {
            spaces: spaces.clone(),
            pg: ordo_conformance::Pg {
                db: pg.db.clone(),
                ..ordo_conformance::Pg::default()
            },
            page_size: 50,
        }),
        registry: deployment.clone(),
    });
    // A tenant's own CORS origins (`design-public-path.md` §8). Off until
    // the deployment owner sets `ORDO_CORS_TENANT_ORIGINS_MAX`.
    let tenant_origins = {
        let env = |k: &str| std::env::var(k).ok();
        let max = crate::tenant_origins::max_from_vars(&env)
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
        let map = Arc::new(crate::tenant_origins::TenantOriginMap::new(
            max,
            pool.clone(),
        ));
        map.ensure_table()
            .await
            .map_err(|e| BootRefusal::Database(e.to_string()))?;
        map.load()
            .await
            .map_err(|e| BootRefusal::Database(e.to_string()))?;
        map
    };
    // Proxy mode (`crate::proxy`): off until the deployment owner names the
    // header and the proxies trusted to set it. Slows a busy visitor down,
    // never refuses one.
    let pacer = {
        let env = |k: &str| std::env::var(k).ok();
        crate::proxy::from_vars(&env)
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?
            .map(|(settings, trusted)| {
                println!(
                    "proxy mode: on, reading {} from {} (slows after {} tokenless requests a \
                     minute; never refuses)",
                    settings.header,
                    settings.trusted.join(", "),
                    settings.slow_after
                );
                Arc::new(crate::proxy::Pacer::new(
                    settings,
                    trusted,
                    Some(metrics.clone()),
                ))
            })
    };
    let redeem_hook: crate::credential_ops::RedeemHook = Arc::default();
    // Backend functions: the app secrets, sealed under the deployment's own
    // keys in the control database, bound by the serving program.
    let secrets = Arc::new(
        crate::secrets::SecretStore::pg(pool.clone(), keys.clone(), &space.0)
            .following(deployment.clone()),
    );
    secrets
        .ensure_table()
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;
    let platform_ops = platform_ops_for(PlatformWiring {
        metrics: &metrics,
        replica: &replica,
        session_policy: auth.as_ref().and_then(|(a, _)| a.policy.clone()),
        guess_limits: auth.as_ref().map(|(a, _)| a.guess.clone()),
        contact: auth.as_ref().and_then(|(a, _)| a.contact().cloned()),
        console_keys: auth.as_ref().and_then(|(a, _)| a.console_keys().cloned()),
        secrets: &secrets,
        origins: tenant_origins.clone(),
        redeem_hook: redeem_hook.clone(),
        second_levels: &second_levels,
        recovery: recovery_handle.clone(),
        recovery_policy: cfg.recovery.clone(),
        provisioning: cfg.provisioning.clone(),
        acl: Some(acl),
        pool: &pool,
        keys: &keys,
        access: &access,
        delegations: &delegations,
        pools: spaces.pools(),
        spaces: &spaces,
        exit_bundles,
        import_bundles,
        blob_door,
        attest: signer_for_workers.is_some(),
        home: space.clone(),
        stores: stores.clone(),
        pipeline,
        serving: Some(&deployment),
        credentials: &credentials,
        buckets,
        members: Some(MemberWiring {
            token_secret: cfg.token_secret,
            metrics: &metrics,
            deployment: &deployment,
            directory: match &cfg.auth {
                Some(_) => Some(Arc::new(
                    crate::principal_directory::PgDirectoryStore::open_on(store_pool.clone())
                        .map_err(|e| BootRefusal::Database(e.to_string()))?,
                )),
                None => None,
            },
            tickets: match &cfg.auth {
                Some(_) => {
                    crate::tickets::ensure_table(&pool)
                        .await
                        .map_err(|e| BootRefusal::Database(e.to_string()))?;
                    let store = crate::tickets::PgTicketStore::open_on(store_pool.clone())
                        .map_err(|e| BootRefusal::Database(e.to_string()))?;
                    Some(Arc::new(
                        crate::tickets::Tickets::new(Arc::new(store))
                            .with_recorder(access.recorder.clone()),
                    ))
                }
                None => None,
            },
            // The same switch as the identity-enrolment policy above, read
            // as what it now is: the ceiling. A deployment with no
            // authentication has no sign-in to admit anybody through, and
            // gets the narrower one.
            ceiling: crate::admission::Ceiling::from_auto_onboard(
                cfg.auth.as_ref().is_some_and(|a| a.auto_onboard),
            ),
            credentials: match (&cfg.auth, &revocations) {
                (Some(a), Some(revocations)) => Some(CredentialWiring {
                    store: Arc::new(
                        crate::credentials::PgCredentialStore::open_on(store_pool.clone())
                            .map_err(|e| BootRefusal::Database(e.to_string()))?,
                    ),
                    revocations: revocations.clone(),
                    session_max_secs: a.session_max_secs,
                    // Off unless the deployment says so: an administrator
                    // typing somebody's password is the less secure shape
                    // and the owner ruled it a recorded option (A4).
                    allow_temporary: match std::env::var("ORDO_AUTH_TEMPORARY_PASSWORDS")
                        .ok()
                        .as_deref()
                        .map(str::trim)
                    {
                        None | Some("") | Some("off") => false,
                        Some("allow") => true,
                        Some(other) => {
                            return Err(BootRefusal::Invalid {
                                var: "ORDO_AUTH_TEMPORARY_PASSWORDS",
                                detail: format!("`{other}` is not `allow` or `off`"),
                            })
                        }
                    },
                }),
                _ => None,
            },
        }),
    })
    .await?;

    // Kept for the sign-in routes; `platform_ops` moves into the door below.
    let enrolment_ops = (
        platform_ops.admission.clone(),
        platform_ops.credentials.clone(),
    );
    // The membership commands' store, for the hold a tenant may put on a
    // recovered person (R6).
    let members_for_hold = platform_ops.members.clone();
    // The sign-in hook of self-serve provisioning gives a person a tenant
    // through the same `TenantOps` the platform op uses (P2).
    let tenants_for_signup = platform_ops.tenants.clone();
    // The share routes call the same `ShareOps` the platform ops do (G3).
    let share_for_auth = platform_ops.share.clone();
    let release_for_auth = platform_ops.release.clone();
    let links_for_auth = platform_ops.links.clone();
    // The agent's half of the consent ceremony (11-4b) runs the same state.
    let consent_for_auth = platform_ops.consent.clone();
    // Tenant-owned sign-in providers (`plan-tenant-providers.md`): one state
    // for the four ops and the sign-in routes. Wired whenever there is
    // authentication, so the ops answer, off, with the setting to change.
    if let (Some((state, _)), Some(a), Some(members)) =
        (&auth, cfg.auth.as_ref(), platform_ops.members.clone())
    {
        if let (Some(directory), Some(ceremonies)) =
            (members.directory.clone(), state.ceremonies.clone())
        {
            let public_url = a.tenant_providers.clone();
            let unused = || {
                crate::federation::parse_return_urls("https://unused.invalid", false)
                    .expect("a fixed address parses")
            };
            let mut tc = crate::tenant_providers::TenantConnections::new(
                public_url.is_some(),
                members,
                directory,
                keys.clone(),
                ceremonies,
                crate::oidc::EgressFactory::deployment(
                    a.egress_roots.clone(),
                    a.egress_deny.for_tenant_providers(),
                ),
                public_url.as_deref().unwrap_or("https://unused.invalid"),
                a.returns.clone().unwrap_or_else(unused),
                &space.0,
            );
            if let Some(admission) = enrolment_ops.0.clone() {
                tc = tc.with_admission(admission);
            }
            tc = tc
                .with_oidc_defaults(a.oidc_defaults.clone())
                .with_lock_wait_ms(a.tenant_lock_wait_ms);
            let tc = Arc::new(tc);
            let _ = platform_ops.tenant_connections.set(tc.clone());
            state.wire_tenant_connections(tc);
        }
    }

    // 17n M2: `PgQuotas` when the ledger is on, `NoQuota` otherwise — a
    // deployment with no `$quotas` row is never refused either way, since
    // `PgQuotas::check` starts with nothing configured and only gates what
    // `ordo quota set` has actually populated.
    let quotas: Option<Arc<crate::quotas::PgQuotas>> = metrics
        .usage_enabled()
        .then(|| Arc::new(crate::quotas::PgQuotas::new(metrics.clone())));
    let quota_gate: Arc<dyn ordo_api::QuotaGate> = match &quotas {
        Some(q) => q.clone(),
        None => Arc::new(ordo_api::NoQuota),
    };

    // The public read path's cache and CDN hook (`design-public-path.md` §3,
    // §9). Read at every boot; a bad value refuses the boot by name.
    let public_path = {
        let env = |k: &str| std::env::var(k).ok();
        let settings = crate::public::settings_from_vars(&env)
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
        let deny = ordo_egress::DenyList::from_vars(&env)
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
        let read_file = |path: &str| std::fs::read_to_string(path).map_err(|e| e.to_string());
        let purge = crate::public::purge_from_vars(&env, &read_file, deny.clone())
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
        let human = crate::public::human_from_vars(&env, &read_file, deny)
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?;
        ordo_api::PublicPath::with_hooks(
            settings,
            ordo_api::PublicHooks {
                purge,
                human,
                monitor: Some(Arc::new(crate::public::MetricsMonitor(metrics.clone()))),
            },
        )
    };
    let api = ApiState {
        registry: deployment.handle(),
        // Metered outside, platform ops inside, so a platform op's duration
        // is timed like any other op's. With the order reversed the
        // platform executor answers `std.space.*` before the meter is
        // reached, and those ops would never appear in the histograms.
        executor: Arc::new(crate::metrics::MeteredExecutor::new(
            Arc::new(crate::platform_ops::PlatformExecutor::new(
                Arc::new(crate::FixtureExecutor {
                    spaces: spaces.clone(),
                    // The `Pg` handle `FixtureExecutor` still carries. It is
                    // used by nothing on the dispatch path — see the note on
                    // the field — and the door's access question goes
                    // through `SwappableAccess`, never through
                    // `MemberAccess`.
                    pg: ordo_conformance::Pg {
                        db: pg.db.clone(),
                        ..ordo_conformance::Pg::default()
                    },
                    page_size: 50,
                }),
                platform_ops,
            )),
            metrics.clone(),
        )),
        access: access.clone(),
        quota: quota_gate,
        bus,
        replay,
        attest,
        page_key,
        sync,
        live_streams: ordo_api::LiveStreams::new(ordo_api::LiveSettings::from_env()),
        refine_limits: ordo_api::RefineLimits::from_env(),
        public: public_path,
    };

    let platform = PlatformState::new(
        access.clone(),
        deployment.clone(),
        pool.clone(),
        keys.clone(),
        space.0.clone(),
        metrics.clone(),
    )
    .with_security_drain(secdrain.health())
    // 17aa: the resolver readiness reads its quarantine register from, so
    // a space the boot held closed is named in the body rather than left
    // for a customer to report.
    .with_spaces(spaces.clone())
    // 7d(b): `/security/events` reads a space's stream from **that space's**
    // database, because S4 routes it there. `spaces.pools()` is the door's
    // own resolver, which is the point: a second resolver here could route a
    // read somewhere the door does not, and then the record an owner is
    // shown is not the record their space keeps.
    .with_pools(spaces.pools().clone())
    .with_access_unread(access.clone())
    // 17t: `/health/ready`'s "blob store, when the program needs one"
    // check asks whether one is *attached*, not whether `ORDO_BLOB_STORE`
    // parsed — `blob_label` is `Some` only once `blob_from_env` produced a
    // working `BlobBinding` above.
    .with_blob_label(blob_label.clone())
    .with_credential_labels(credential_labels);
    // What enrolment adds to the sign-in routes: tickets, the admission a
    // sign-in that names a tenant runs, and the credential ops a forced
    // password change needs. They are the platform ops' own, so a ticket
    // redeemed at sign-in is the same store `std.space.createPerson` issued
    // into.
    if let Some((state, _)) = &auth {
        let tickets = enrolment_ops
            .0
            .as_ref()
            .and_then(|a| a.tickets.clone())
            .or_else(|| enrolment_ops.1.as_ref().map(|c| c.tickets.clone()));
        state.wire_enrolment(crate::auth::Enrolment {
            tickets: tickets.clone(),
            admission: enrolment_ops.0.clone(),
            credentials: enrolment_ops.1.clone(),
        });
        // P4: a person's own list of tenants, read from the door's live
        // engine so a removal or a suspension is as absent here as there.
        {
            let live = access.clone();
            state.wire_spaces(Arc::new(crate::auth::spaces::TenantLister {
                engine: Arc::new(move || live.current().engine.clone()),
                pool: Some(pool.clone()),
            }));
        }
        if let Some(share) = share_for_auth {
            state.wire_sharing(share);
        }
        // 27aq: deleting one's own account, refused while any tenant's
        // release rule still holds the person.
        if let Some(release) = release_for_auth {
            let live = access.clone();
            // Whether, and when, a deleted account's data is removed: the app
            // developer's `ORDO_ACCOUNT_REMOVAL`, default never (2026-10-06).
            let removal = crate::account_removal::Removal::from_env().map_err(|detail| {
                BootRefusal::Invalid {
                    var: crate::account_removal::REMOVAL_ENV,
                    detail,
                }
            })?;
            state.wire_account(Arc::new(crate::auth::account::Accounts {
                release,
                engine: Arc::new(move || live.current().engine.clone()),
                removal,
            }));
        }
        if let Some(links) = links_for_auth {
            state.wire_links(links);
        }
        if let Some(consent) = consent_for_auth {
            state.wire_consent(consent);
        }
        if let Some(tenants) = tenants_for_signup {
            state.wire_signup(Arc::new(crate::self_serve::Signup::new(
                crate::self_serve::SelfServeStore::new(pool.clone()),
                tenants,
            )));
        }
        // Credential recovery (`plan-passkey-recovery.md`): its cases in
        // `$recovery`, its tickets in the enrolment tickets' store, its acts
        // in the door's own recorder and mirrored into each tenant the
        // person belongs to.
        // The factors' store, opened once: recovery removes them when it
        // completes and the second step reads and writes them.
        let second_store: Arc<dyn crate::second_factor_store::SecondFactorStore> = Arc::new(
            crate::second_factor_store::PgSecondFactorStore::open_on(store_pool.clone())
                .map_err(|e| BootRefusal::Database(e.to_string()))?,
        );
        if let Some(tickets) = tickets {
            let cases = crate::recovery::PgRecoveryStore::open_on(store_pool.clone())
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            let door = access.clone();
            let door_for_hold = access.clone();
            let door_for_roles = access.clone();
            let codes = crate::recovery_codes::PgCodeStore::open_on(store_pool.clone())
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            let policy = cfg.recovery.clone();
            let mut service =
                crate::recovery::Recovery::new(Arc::new(cases), tickets, policy.clone());
            // A recovery removes every second factor once it completes (owner,
            // 2026-09-29), and not when it is opened.
            service =
                service.with_settlement(Arc::new(crate::second_factors::RemoveSecondFactors {
                    store: second_store.clone(),
                    passkey: state.passkey.clone(),
                    recorder: Some(access.recorder.clone()),
                }));
            // A contest puts back the sign-in settings a person can change
            // (second-step level, contact email) to their value when the
            // case opened (owner, 2026-10-25).
            {
                let mut settings: Vec<Arc<dyn crate::recovery_settings::PersonSetting>> =
                    vec![Arc::new(crate::recovery_settings::SecondStepLevel(
                        second_store.clone(),
                    ))];
                if let Some(emails) = state.contact().cloned() {
                    settings.push(Arc::new(crate::recovery_settings::ContactEmailSetting {
                        emails,
                        runtime: tokio::runtime::Handle::current(),
                    }));
                }
                let snapshots = crate::recovery_settings::PgSnapshots::open_on(store_pool.clone())
                    .map_err(|e| BootRefusal::Database(e.to_string()))?;
                service =
                    service.with_settlement(Arc::new(crate::recovery_settings::RestoreSettings {
                        snapshots: Arc::new(snapshots),
                        settings,
                        keep_secs: policy
                            .contest_secs
                            .saturating_mul(2)
                            .saturating_add(90 * 86_400),
                    }));
            }
            // A tenant that holds suspends a recovered person once the
            // recovery completes (R6).
            if let Some(members) = members_for_hold.clone() {
                service = service.with_settlement(Arc::new(crate::recovery_hold::HoldOnRecovery {
                    members,
                    tenants: crate::recovery_hold::TenantRecovery::new(
                        pool.clone(),
                        policy.hold_default,
                    ),
                    tenants_of: Box::new(move |person| {
                        door_for_hold
                            .current()
                            .engine
                            .spaces_of(&ordo_identity::PrincipalId(person.to_string()))
                            .into_iter()
                            .map(|(space, _, _)| space.0)
                            .collect()
                    }),
                    runtime: tokio::runtime::Handle::current(),
                }));
            }
            // A tenant may switch recovery codes off for the people it created.
            if enrolment_ops.1.is_some() {
                service =
                    service.with_codes_gate(Arc::new(crate::recovery_hold::TenantCodesGate {
                        managed: crate::credential_ops::ManagedStore::new(pool.clone()),
                        tenants: crate::recovery_hold::TenantRecovery::new(
                            pool.clone(),
                            policy.hold_default,
                        ),
                        runtime: tokio::runtime::Handle::current(),
                    }));
            }
            // A person whose only sign-ins belong to an outside provider is
            // recovered by that provider, so no case is opened for them.
            let outside_dir: Arc<dyn ordo_identity::DirectoryStore> = Arc::new(
                crate::principal_directory::PgDirectoryStore::open_on(store_pool.clone())
                    .map_err(|e| BootRefusal::Database(e.to_string()))?,
            );
            let recovery = Arc::new(
                service
                    .with_directory(outside_dir)
                    .with_recorder(access.recorder.clone())
                    .with_codes(Arc::new(
                        crate::recovery_codes::RecoveryCodes::new(Arc::new(codes))
                            .with_set_size(policy.codes_per_set),
                    ))
                    .with_tenants(Box::new(move |person| {
                        door.current()
                            .engine
                            .spaces_of(&ordo_identity::PrincipalId(person.to_string()))
                            .into_iter()
                            .map(|(space, _, _)| space.0)
                            .collect()
                    }))
                    // A case about a tenant's owner reaches its managers.
                    .with_roles(crate::recovery::roles_from(door_for_roles)),
            );
            // The deployment owner's recovery ops reach it through this
            // handle, and the sign-in routes through the auth state.
            let _ = recovery_handle.set(recovery.clone());
            state.wire_recovery(recovery);
        }
        // The second step (`plan-second-factor.md`): its factors in
        // `$secondfactor`, a code-generating factor's secret sealed under the
        // deployment's own custody.
        {
            let factors = second_store.clone();
            let second = cfg.second_factor.clone();
            let factors_for_keys = factors.clone();
            let factors_for_policy = factors.clone();
            let totp = Arc::new(crate::second_factors::TotpMethod::new(
                factors.clone(),
                crate::second_factors::Sealer::new(keys.clone()),
                second.clone(),
            ));
            let mut methods = crate::second_factors::SecondFactors::new(second.clone())
                .with_levels(second_levels.clone())
                .with_store(factors_for_policy)
                .with_totp(totp);
            // The same backup codes recovery issues stand in for a lost factor.
            if let Some(codes) = state.recovery().and_then(|r| r.codes().cloned()) {
                methods = methods.with_backup(Arc::new(
                    crate::second_factors::BackupCodeMethod::new(codes, factors, &second),
                ));
            }
            // A security key as a second factor needs the passkey provider: the
            // same verifier and the same `$passkey` table, told apart by role.
            if let Some(passkey) = state.passkey.clone() {
                methods =
                    methods.with_key(Arc::new(crate::second_factors::SecurityKeyMethod::new(
                        factors_for_keys,
                        passkey,
                        second.clone(),
                    )));
            }
            state.wire_second_factors(Arc::new(methods));
            // A credential reset removes them too, when the person redeems it.
            let (hook_store, hook_passkey) = (second_store.clone(), state.passkey.clone());
            let _ = redeem_hook.set(Arc::new(move |principal: &str| {
                crate::second_factors::remove_all_for(
                    hook_store.as_ref(),
                    hook_passkey.as_ref(),
                    principal,
                )
            }));
        }
    }
    let platform = match &auth {
        Some((state, _)) => platform.with_auth(state.clone()),
        None => platform,
    };

    let catchup = crate::tenant_forward::CatchUp::new(spaces.clone(), pool.clone());
    // What this node serves, in `$nodes`, so `ordo apply` can wait for it.
    let beat_secs: u64 = parsed(
        crate::node_status::BEAT_SETTING,
        crate::node_status::DEFAULT_BEAT_SECS,
    )?;
    if beat_secs == 0 {
        return Err(BootRefusal::Invalid {
            var: crate::node_status::BEAT_SETTING,
            detail: "a zero-second beat never sleeps between writes".into(),
        });
    }
    let node_status = crate::node_status::NodeStatus::new(
        pool.clone(),
        replica.clone(),
        Duration::from_secs(beat_secs),
    );
    catchup.report_to(node_status.clone());
    // The program's routines (checklist 12a). Empty wiring for a program
    // that declares none, so the trigger workers stay off and say why.
    let triggers = {
        let live = deployment.current();
        crate::routines::wiring(
            &live.fixture.schema,
            &live.version,
            spaces.clone(),
            &space.0,
            cfg.mail.is_some(),
        )
    };
    let routines = Arc::new(crate::routines::LiveRoutines::following(
        triggers.clone(),
        spaces.clone(),
        space.0.clone(),
        cfg.mail.is_some(),
    ));
    if cfg.mail.is_some() {
        crate::mail::ensure_table(&pool)
            .await
            .map_err(|e| BootRefusal::Database(e.to_string()))?;
    }
    // A person's own download (owner ruling 2026-10-22). Off until the
    // program marks an op `download`; read from the live version per call.
    let download = ordo_api::download::DownloadState::new(
        Arc::new(crate::own_download::ProgramDownload {
            deployment: deployment.clone(),
            stream: Some(access.recorder.clone()),
            now: Arc::new(now_secs),
        }),
        ordo_api::download::DownloadLimits::from_env(),
    )
    .open_to_delegated(cfg.delegable.opens(ordo_api::download::DOWNLOAD_NAME));
    let pool_for_functions = pool.clone();
    let secrets_for_functions = secrets.clone();
    // The deny list mail and attestation use: the categories plus
    // `ORDO_EGRESS_DENY_CIDRS` and `ORDO_EGRESS_PRIVATE_HOSTS`.
    let functions_deny = {
        let env = |k: &str| std::env::var(k).ok();
        ordo_egress::DenyList::from_vars(&env)
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?
    };
    // `ORDO_FN_BODY_BYTES`, `ORDO_FN_BLOB_BYTES` and
    // `ORDO_FN_FETCH_BODY_BYTES`: a call's body, the bytes one call may read
    // through `ctx.blob`, and the body one `fetch` may send.
    let functions_settings = {
        let env = |k: &str| std::env::var(k).ok();
        crate::functions::FnSettings::from_vars(&env)
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?
    };
    // A function reads files through the blob door's own port, as the
    // person calling. No store, no port: `ctx.blob` answers `blob_no_store`.
    let functions_blobs = blobs.as_ref().map(|b| b.port.clone());
    // `ORDO_FN_CACHE_DIR`: where the compiled function engine is kept.
    let functions_engine_cache = {
        let env = |k: &str| std::env::var(k).ok();
        crate::functions::engine_cache_from_vars(&env)
            .map_err(|(var, detail)| BootRefusal::Invalid { var, detail })?
    };
    Ok(Runtime {
        mail: cfg.mail.clone(),
        custody_label: custody_choice.label(),
        auth_label: cfg.auth.as_ref().map(|a| {
            format!(
                "{} over $credential; session {}s, ceiling {}s; {}",
                a.methods.join(", "),
                a.session_secs,
                a.session_max_secs,
                if a.auto_onboard {
                    "auto-onboard"
                } else {
                    "invite-only"
                }
            ) + &a
                .email
                .as_ref()
                .map(|p| format!("; {}", p.describe()))
                .unwrap_or_default()
        }),
        recovery_label: cfg.auth.as_ref().map(|_| cfg.recovery.describe()),
        second_step_label: cfg.auth.as_ref().map(|_| cfg.second_factor.describe()),
        second_levels: second_levels.clone(),
        passkey_label: cfg
            .auth
            .as_ref()
            .and_then(|a| a.relying_party.as_ref())
            .map(|rp| {
                format!(
                    "relying party {} ({}); {} origin(s) under it{}",
                    rp.rp_id(),
                    rp.name(),
                    rp.origins().len(),
                    if rp.related_origins().is_empty() {
                        String::new()
                    } else {
                        format!(
                            ", {} related origin(s) on other domains (publish \
                             /.well-known/webauthn on {})",
                            rp.related_origins().len(),
                            rp.rp_id()
                        )
                    }
                )
            }),
        apex_domain,
        cors,
        tenant_origins,
        pacer,
        cfg_addr: addr,
        shutdown_grace,
        space,
        system_space,
        deployment_owners: cfg.deployment_owners.clone(),
        pg,
        witness,
        reconciled,
        pool,
        keys,
        credentials,
        deployment,
        access,
        metrics,
        api,
        platform,
        secdrain,
        security_checkpoint,
        token_secret,
        delegations,
        pg_bus,
        disabled,
        blob_label,
        blobs,
        download,
        stores,
        triggers,
        routines,
        checkpoint_policy: ordo_exec::cadence::LogPolicy::DEFAULT,
        checkpoint_signer: signer_for_workers,
        replica,
        usage_flush,
        usage_rollup,
        catchup,
        node_status,
        functions: Arc::new(
            crate::functions::HostFunctions::new(
                ordo_fn::HostConfig {
                    engine_cache: functions_engine_cache,
                    ..ordo_fn::HostConfig::default()
                },
                crate::functions::FnTable::default(),
                spaces.clone(),
                pool_for_functions,
                crate::secrets::fn_fetch(
                    secrets_for_functions.clone(),
                    ordo_egress::TlsRoots::webpki(),
                    functions_deny,
                    functions_settings.fetch_limits(),
                ),
                secrets_for_functions,
            )
            .with_blobs(functions_blobs)
            .with_settings(functions_settings),
        ),
        spaces,
        quotas,
        secrets,
    })
}

/// **What a deployment repaired by hand finds when it comes back up**
/// (17aa, and the owner's *"there must be some reconciliation process on
/// startup if it was broken and needed manual intervention to fix"*).
///
/// Every space this boot holds a name for — the system space, whose id is a
/// constant compiled into the binary, and `ORDO_SPACE` — has its chain read
/// and compared against the control database's witness. Attached spaces are
/// not enumerated here and do not need to be: they are reconciled by the
/// first request that names one, through the same function, and a space no
/// request ever names has nothing to hold open.
///
/// # What it settles on its own
///
/// One thing: a difference somebody already wrote down. A chain that
/// contradicts a recorded head, where a restore marker attests the
/// difference and the chain reaches that marker, is `Explained` and the
/// space serves. That is reconciliation completing the loop a recorded
/// restore opened, and it is the reason recording one through the door
/// works: the marker written into a running deployment is the marker the
/// next boot reads.
///
/// # What it will not do
///
/// It will not write a marker, infer one, or decide that a rewind looks
/// plausible enough to serve. **Silently accepting a rewound chain because
/// it looks reasonable is the act the whole restore mechanism exists to
/// prevent**, and a reconciliation that did it would make every guarantee
/// below it decorative. So an unexplained rewind holds the space closed and
/// names the two lawful remedies, and a continuity read that *errors* holds
/// it closed too, because "I could not check" is not "it checks out".
///
/// It also repairs nothing in the database. Missing rows stay missing,
/// custody it cannot open stays unopened, and a half-applied schema stays
/// half-applied. The operator's acts are the operator's; this reads, says
/// what it found, and decides which spaces serve.
///
/// Returns one line per space, for the startup report and for the startup
/// report only — the refusals themselves live in the resolver's quarantine
/// register, which `/health/ready` reads for as long as the process runs.
async fn reconcile_at_boot(
    spaces: &Arc<crate::spaces::Spaces>,
    home: &str,
) -> Vec<(String, crate::spaces::Reconciled)> {
    let mut names = vec![crate::provision::SYSTEM_SPACE.to_string()];
    if home != crate::provision::SYSTEM_SPACE {
        names.push(home.to_string());
    }
    let mut out = Vec::new();
    for name in names {
        let verdict = spaces.reconcile(&name).await;
        out.push((name, verdict));
    }
    out
}

/// The features that are off, and the variable that would turn each on.
///
/// A feature that is off and unmentioned is a feature an operator believes
/// is on. This is `/health/deep`'s `wired: false` posture moved to the
/// moment somebody is looking.
fn census(cfg: &RuntimeConfig) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if cfg.checkpoint.is_none() {
        out.push(("checkpoint".into(), "ORDO_CHECKPOINT_KEY is not set".into()));
        out.push(("attest".into(), "ORDO_CHECKPOINT_KEY is not set".into()));
        out.push((
            "std.space.attestTo, std.space.chainProof".into(),
            "ORDO_CHECKPOINT_KEY is not set".into(),
        ));
    }
    if cfg.exit_bundles.is_none() {
        out.push((
            "std.space.export, std.space.delete".into(),
            "ORDO_EXIT_BUNDLE_DIR is not set".into(),
        ));
    }
    // Its own line rather than the one above, because the two directories
    // answer different questions and a deployment can legitimately serve
    // one and not the other: a hosted tenant is meant to be able to leave
    // long before an operator is willing to let anyone stage bytes on the
    // deployment's disk.
    if cfg.import_bundles.is_none() {
        out.push((
            "std.space.import".into(),
            "ORDO_IMPORT_BUNDLE_DIR is not set, so a bundle has nowhere to be staged".into(),
        ));
    }
    // Registered from U6 on a node that honours an attachment, so this
    // line is now about the nodes that do not — and it names which part of
    // `buckets_wired` is missing, because "the op is off" with no reason is
    // what sends an operator to read the source.
    if !buckets_wired(cfg) {
        out.push((
            "std.space.attachBucket".into(),
            if !matches!(cfg.blob_routing, BlobRouting::Attached) {
                "ORDO_BLOB_ROUTING is `single`: every space resolves to the one \
                 ORDO_BLOB_STORE, so an attachment would be recorded and never honoured"
                    .to_string()
            } else if cfg.blob.is_none() {
                "ORDO_BLOB_STORE is not set: there would be no store for a space with no \
                 attachment to resolve to"
                    .to_string()
            } else {
                "ORDO_TENANT_CREDENTIALS installs no credential, so no attachment could \
                 name material this node holds"
                    .to_string()
            },
        ));
    }
    if !cfg.security_checkpoint {
        out.push((
            "secevent.retain".into(),
            "there is no security checkpoint key: ORDO_SECURITY_CHECKPOINT_KEY does not parse, \
             or the one made at first boot could not be opened (see the boot log)"
                .into(),
        ));
        // Its own line, because the consequence is not the retention one. No
        // retention means the stream grows; no checkpoint means nothing signs
        // the head, so `ordo verify` cannot tell whether entries were removed
        // from the END of a stream — the one property a chain walk cannot
        // establish on its own.
        out.push((
            "secevent.checkpoint".into(),
            "there is no security checkpoint key (ORDO_SECURITY_CHECKPOINT_KEY, see the boot \
             log), so nothing signs the head of any \
             security stream and entries removed from the END of one are undetectable"
                .into(),
        ));
    }
    if cfg.auth.is_none() {
        out.push((
            "std.space.share".into(),
            "ORDO_AUTH_METHODS is not set: a share is redeemed by a person signing in".into(),
        ));
    }
    match &cfg.auth {
        None => out.push((
            "/auth/begin, /auth/complete, /auth/logout, /auth/refresh".into(),
            "ORDO_AUTH_METHODS is not set: this deployment verifies tokens it cannot issue".into(),
        )),
        // Configured, and still owed. Passkey sign-in, registration,
        // enrolment, recovery and the second factor are served (2026-09-29);
        // what is not is the outside providers and emailed codes of
        // plan-auth-federated.md, and the census says so rather than a user.
        Some(_) => {
            out.push((
                "SAML sign-in".into(),
                "not built: OAuth2 connections (Discord, GitHub), OIDC connections (Google, \
                 Microsoft, any issuer with discovery) and emailed codes are served; SAML is \
                 plan-auth-federated.md L2 and L3"
                    .into(),
            ));
        }
    }
    out
}

/// Wrap the custody [`build_custody`] returned in the metering decorator,
/// with the deployment's security stream attached.
///
/// The one place the runtime meters custody. `assemble_program` calls it
/// before any component takes a copy of the keys, so the executor, the
/// replay cache, the workers, the blob pipeline, the share links and every
/// later deploy hold the same metered handle. A provider wrapped twice
/// would count and record each release twice, which is why this is a
/// function with one caller rather than a pattern repeated at each holder.
fn meter_custody(
    keys: Arc<dyn KeyProvider>,
    metrics: &Arc<Metrics>,
    recorder: &Arc<ordo_secevent::Recorder>,
) -> Arc<dyn KeyProvider> {
    Arc::new(crate::metrics::MeteredKeys::new(keys, metrics.clone()).with_stream(recorder.clone()))
}

/// `local` over the Postgres wrapped-key store; `test` over the constant.
///
/// **A tenant is never provisioned as a boot side effect.**
/// `LocalKeyProvider::release` refuses an unknown space rather than
/// silently creating keys, so a typo cannot mint a tenant, and a binary
/// that provisioned whatever `ORDO_SPACE` said would turn a typo into a
/// brand-new tenant with brand-new keys, serving an empty space that looks
/// perfectly healthy. So an unknown tenant is a refusal with the command
/// that fixes it ([`BootRefusal::UnprovisionedTenant`]).
///
/// # The one space that is created here, and how it is told apart
///
/// [`crate::provision::SYSTEM_SPACE`] is the platform's own home
/// (checklist 17v). Its custody is minted on the way up, unconditionally
/// and idempotently, because a deployment without it has nowhere to keep
/// what it knows about itself and cannot be told to create it later by a
/// command nobody has run yet.
///
/// The two cases are told apart by **where the name came from**: the
/// system space's id is a constant compiled into this binary
/// ([`ordo_exec::erase::SYSTEM_SPACE`], which is also what
/// `ordo_exec::erase::is_system_space` refuses to erase), read from no
/// input at all, while
/// a tenant's id arrives in `ORDO_SPACE`, in a request header, or on an
/// operator's command line. So the space this function creates unasked is
/// the one whose name it was holding before the process started, and no
/// value an operator can mistype reaches it. A typo in `ORDO_SPACE` still
/// refuses, which is the property the old rule existed for.
///
/// **What the first boot fixes about the master key.** Custody minted here
/// is wrapped under whatever master this boot opened. If that master was
/// wrong, the blind-index probe below refuses (for the deployment's own
/// space) and `ordo keys rewrap-master` is the way back; a first boot under
/// a mistyped master is a deployment whose system space is wrapped under it,
/// and the remedy is the same rewrap.
fn build_custody(
    cfg: &RuntimeConfig,
    store_pool: Pool,
) -> Result<Arc<dyn KeyProvider>, BootRefusal> {
    let Some(source) = cfg.custody.source() else {
        return Ok(Arc::new(ordo_crypto::TestProvider::with_space(
            &cfg.space.0,
        )));
    };
    let master = source.open().map_err(BootRefusal::Custody)?;
    let store =
        ordo_exec::keystore::PgKeyStore::open_on(store_pool).map_err(BootRefusal::Custody)?;

    // `plan-production-ops.md` R1b: `$keycustody` holds wrapped keys, never
    // the master — that is what makes the table safe to sit inside the
    // deployment's own backup. A row whose `wrapped` bytes equal the master
    // itself, or the key `derive_from_secret` would produce from it, is
    // that misconfiguration landing anyway, and the boot refuses rather
    // than serving under a master a copy of the database already contains.
    if store.holds_bytes(&master)
        || store.holds_bytes(&ordo_crypto::master::derive_from_secret(&master))
    {
        return Err(BootRefusal::Custody(ordo_crypto::KeyError::refused(
            "the master key is stored inside `$keycustody` — wrapped keys belong in the \
             backup, the master does not\n  remedy: remove the offending row, then rotate the \
             master with `ordo keys rewrap-master`",
        )));
    }

    // `from_master`, never `new`: the source returns the master itself, and
    // `new` would hash it again into a provider that opens nothing.
    let provider = ordo_crypto::LocalKeyProvider::from_master(master, Box::new(store));

    // The platform's own home, before the check below, so that a deployment
    // whose `ORDO_SPACE` *is* the system space comes up on an empty
    // database. Idempotent: a space that already has an SMK keeps the one
    // it has.
    provider
        .create_space(&ordo_crypto::SpaceId(
            crate::provision::SYSTEM_SPACE.to_string(),
        ))
        .map_err(BootRefusal::Custody)?;

    // **A tenant with no custody no longer stops the boot** (17ac). It was
    // the last refusal of the shape 17aa removed everywhere else: one
    // space's state deciding whether a whole deployment starts. The
    // deployment comes up, `reconcile_at_boot` finds the space has no
    // custody, it is quarantined with the remedy as its reason, and
    // `/health/ready` names it. Every other tenant and the control plane
    // serve throughout.
    //
    // The property the refusal was protecting is untouched and was never
    // the refusal's to hold: `current_epoch` reads and never creates, so a
    // typo in `ORDO_SPACE` still mints nothing for the name that was
    // mistyped.
    // …**and this master must be the one that wrapped it.** `current_epoch`
    // above is a presence check — it takes the maximum epoch of the store's
    // DEK rows and unwraps nothing — so a mistyped `ORDO_MASTER_KEY` or the
    // wrong passphrase file passes it and fails at the first sealed read,
    // after the load balancer has already sent traffic. `blind_index`
    // derives the index key from the space's SMK, so it unwraps; it takes
    // no lease and the reserved entity/field names address nothing.
    //
    // **The system space is always probed**, and that is new with 17ac. It
    // was created unconditionally a few lines above, so it always has
    // custody, which makes it the one space a deployment can always ask
    // this of. Probing only `ORDO_SPACE` meant a deployment whose tenant
    // was not provisioned yet never checked its master key at all, and
    // that is now the ordinary first-boot state rather than an error.
    let system = ordo_crypto::SpaceId(crate::provision::SYSTEM_SPACE.to_string());
    let mut probes = vec![system.clone()];
    // The configured space too, when it has custody of its own. A space
    // with none has nothing to probe, and its absence is the quarantine
    // above rather than a master-key fault.
    if cfg.space != system && provider.current_epoch(&cfg.space).is_ok() {
        probes.push(cfg.space.clone());
    }
    for space in probes {
        if let Err(e) =
            provider.blind_index(&space, "$custody", "probe", ordo_crypto::BlindValue::Null)
        {
            return Err(BootRefusal::Custody(ordo_crypto::KeyError::broken(
                format!(
                    "custody for space `{}` exists but this master key does not open it ({e})\n  \
                 the wrapped keys are in `$keycustody`; the master is not, and is not recoverable \
                 from them",
                    space.0
                ),
            )));
        }
    }
    Ok(Arc::new(provider))
}

/// The text Postgres returns for an authentication failure names neither
/// `ORDO_PG_PASSWORD` nor the fact that this client never reads
/// `PGPASSWORD`. Both are exactly what the operator needs.
fn with_password_hint(text: &str) -> String {
    let auth = text.contains("password")
        || text.contains("authentication")
        || text.contains("28P01")
        || text.contains("28000");
    if auth {
        format!(
            "{text}\n  set ORDO_PG_PASSWORD (or ORDO_PG_PASSWORD_FILE) if this server requires \
             one — the pool does not read PGPASSWORD"
        )
    } else {
        text.to_string()
    }
}

/// The boot refusal for a pool that would not dial. When the server could
/// not be reached at all, the text names `ORDO_PG_HOST:ORDO_PG_PORT` with
/// their values (never the password) and what to check. This is the one
/// place the host turns a connect error into its boot message.
fn connect_refusal(text: &str, host: &str, port: u16) -> BootRefusal {
    let unreachable = [
        "error connecting",
        "refused",
        "timed out",
        "resolve",
        "lookup",
    ]
    .iter()
    .any(|k| text.to_lowercase().contains(k));
    let text = with_password_hint(text);
    if unreachable {
        BootRefusal::Database(format!(
            "{text}\n  could not reach ORDO_PG_HOST:ORDO_PG_PORT, which is {host}:{port}. \
             Check that Postgres is running and listening there, that the name resolves \
             from this process, and that no firewall or network rule blocks the port"
        ))
    } else {
        BootRefusal::Database(text)
    }
}

// ── the startup log ──────────────────────────────────────────────────────

impl Runtime {
    /// One block, once, on stdout, before the listener binds.
    ///
    /// The deployment's answer to *"what did you decide?"*, and it exists
    /// because every posture that is not "refuse to boot" is a silent
    /// difference between two deployments that otherwise look identical.
    ///
    /// **No secret, no derived secret, and no length of a secret.** Custody
    /// prints the provider kind and the current epoch — the same two facts
    /// `/health/deep` already discloses to an owner. The database line
    /// prints user, host and database, never whether a password was
    /// supplied: "password: set" is a fact about the environment of a
    /// process an attacker is already reading.
    pub fn print_startup(&self) {
        let custody = self.custody_label;
        let live = self.deployment.current();
        println!(
            "ordo-host {}  addr={}  space={}",
            env!("CARGO_PKG_VERSION"),
            self.cfg_addr,
            self.space.0
        );
        println!(
            "  database    {}@{}:{}/{}  pool={}  acquire={}ms  tls=off",
            self.pg.user,
            self.pg.host,
            self.pg.port,
            self.pg.db,
            self.pg.size,
            self.pg.acquire_timeout.as_millis()
        );
        let epoch = self
            .keys
            .current_epoch(&self.space)
            .map(|e| e.to_string())
            .unwrap_or_else(|_| "?".into());
        println!("  custody     {custody}  epoch={epoch}");
        println!(
            "  system      {}  ({})",
            crate::provision::SYSTEM_SPACE,
            self.system_space
        );
        // 17aa. One line per space the boot read, and a closed space gets
        // the remedies under it: an operator reading this log is the
        // operator who has to act, and a quarantine they have to go and
        // look up is a quarantine they find out about from a customer.
        for (name, verdict) in &self.reconciled {
            match verdict {
                crate::spaces::Reconciled::Served(c) => {
                    println!("  continuity  {name}: {c}")
                }
                // 17ac. Its own arm, because the remedy differs and a
                // remedy that does not fit the fault is worse than none:
                // an operator told to record a restore for a tenant that
                // was never created goes looking for a backup that does
                // not exist. `why()` carries the command for this one.
                crate::spaces::Reconciled::Unprovisioned { .. } => {
                    println!("  QUARANTINE  {name}: not served — {}", verdict.why());
                    println!(
                        "              the deployment is up and every other space is served; \
                         this one is refused until it exists."
                    );
                }
                other => {
                    println!("  QUARANTINE  {name}: not served — {}", other.why());
                    println!(
                        "              run:  ordo restore --record --space {name} --as <you> \
                         --source …"
                    );
                    println!(
                        "              if this database was restored on purpose; otherwise \
                         restore the missing suffix."
                    );
                    println!(
                        "              every other space is served, and this one is refused \
                         until the restore is recorded."
                    );
                }
            }
        }
        match live.report.as_deref() {
            Some(r) => println!(
                "  program     {}  ops={}  gate: {}",
                live.version,
                live.manifests.len(),
                r.summary()
            ),
            None => println!(
                "  program     (none) — no version is active; run `ordo apply` and \
                 this node will swap it in"
            ),
        }
        println!("  door        identity tokens; delegations: registry present");
        match &self.auth_label {
            Some(l) => println!("  auth        {l}"),
            None => println!("  auth        none (ORDO_AUTH_METHODS is not set)"),
        }
        match &self.mail {
            Some(m) => println!("  mail        {}", m.label),
            None => println!("  mail        none (ORDO_MAIL_KIND is not set)"),
        }
        println!(
            "  platform    {}",
            self.deployment.platform_names().join(", ")
        );
        match &self.blob_label {
            Some(l) => println!("  blobs       {l}"),
            // Not a DISABLED line: for a program with no stored blob field
            // this is not a feature that is off, it is a feature the
            // program never asked for. A program that does ask does not
            // reach here at all — `assemble` refuses.
            None => println!("  blobs       no store configured (this program declares none)"),
        }
        println!(
            "  routines    {}",
            crate::routines::describe(&live.fixture.schema)
        );
        println!("  pausing     {}", crate::pause::current().describe());
        println!("  delegable   {}", crate::delegable::current().describe());
        println!(
            "  profiles    {}",
            crate::access_template::current().describe()
        );
        println!(
            "  consent     {}",
            crate::consent::Settings::from_env()
                .map(|s| s.describe())
                .unwrap_or_else(|(var, e)| format!("{var}: {e}"))
        );
        println!(
            "  leaving     {}",
            crate::release_ops::describe_left_name(
                crate::release_ops::left_name_from_env()
                    .ok()
                    .flatten()
                    .as_deref()
            )
        );
        println!(
            "  accounts    {}",
            crate::account_removal::Removal::from_env()
                .unwrap_or_default()
                .describe()
        );
        println!("  presence    {}", crate::presence::current().describe());
        if let Some(label) = &self.recovery_label {
            println!("  recovery    {label}");
        }
        if let Some(label) = &self.second_step_label {
            println!("  second step {label}");
        }
        match (&self.apex_domain, &self.passkey_label) {
            (_, Some(l)) => println!("  passkey     {l}"),
            (Some(d), None) => println!(
                "  apex        {d} (recorded; `passkey` is not in ORDO_AUTH_METHODS, so nothing uses it)"
            ),
            (None, None) => {}
        }
        for (name, why) in &self.disabled {
            println!("  DISABLED    {name:<16} — {why}");
        }
        let rule = self.metrics.alert_rule();
        println!(
            "  metrics     per-space series: {}   deny-alert: {} in {}s ({})",
            if self.metrics.per_space_series() {
                "on"
            } else {
                "off"
            },
            rule.threshold,
            rule.window_secs,
            if rule.configured {
                "configured"
            } else {
                "built-in default"
            }
        );
        println!(
            "  usage       ledger: {}   replica={}   flush={}s   rollup={}s",
            if self.metrics.usage_enabled() {
                "on"
            } else {
                "off (ORDO_USAGE_LEDGER is not set)"
            },
            self.replica,
            self.usage_flush.as_secs(),
            self.usage_rollup.as_secs(),
        );
    }

    /// The door's signing secret. Not the field itself (kept private so an
    /// accidental `Debug`/log of `Runtime` cannot reach it) — a getter,
    /// for the one caller outside this module that needs it:
    /// `ordo dev`'s own save loop, which drives `Deployment::swap` directly
    /// rather than through the deploy channel and needs
    /// [`refresh_access`]'s same follow-up (`spawn_deploy_listener` is the
    /// other caller, and it is inside this module already).
    pub fn token_secret(&self) -> [u8; 32] {
        self.token_secret
    }

    /// The delegation registry `refresh_access` needs to rebuild the door.
    /// See [`Runtime::token_secret`] for why this is a getter rather than a
    /// public field.
    pub fn delegations(&self) -> Arc<crate::delegation::PgDelegationRegistry> {
        self.delegations.clone()
    }

    /// The invalidation bus, concrete. The public cache's tests reach it to
    /// stand in for a lost listener.
    pub fn pg_bus(&self) -> Arc<crate::invalidation::PgInvalidationBus> {
        self.pg_bus.clone()
    }
}

/// Rebuild the access engine from the version now serving, and install it.
///
/// **Not optional after a swap that changes what is dispatchable.** The
/// engine pins a manifest per op (`spawn_deploy_listener`'s own comment:
/// "a swap changes the ops, and the engine pins a manifest per op"), so a
/// swap without this follow-up leaves the door refusing every op the new
/// version added or changed — `access_denied`, indistinguishable at the
/// wire from a caller who never had permission, because the engine simply
/// has no policy for a name it was built without. `spawn_deploy_listener`
/// runs this inline for a second process's `ordo apply`; `ordo dev`
/// (`plan-dev-loop.md` D3) is the other caller, and its swap is
/// `Deployment::swap` called directly rather than reached through the
/// deploy channel, but the property it needs is identical.
pub async fn refresh_access(
    deployment: &Deployment,
    pool: &Pool,
    token_secret: &[u8; 32],
    metrics: &Arc<Metrics>,
    access: &SwappableAccess,
    delegations: Arc<crate::delegation::PgDelegationRegistry>,
    credentials: &Arc<dyn ordo_exec::route::DbCredentials>,
) -> Result<(), BootRefusal> {
    let wiring = AccessRebuild {
        deployment,
        pool,
        token_secret,
        metrics,
        access,
        delegations,
        credentials,
    };
    rebuild_access(wiring, crate::directory::Scope::All).await
}

/// [`refresh_access`] after a change to one space: reads that space's
/// database again and takes every other tenant database from the cache
/// (reported fix slow-passkey-signup). A new tenant and a membership change
/// come here.
#[allow(clippy::too_many_arguments)]
pub async fn refresh_access_for(
    deployment: &Deployment,
    pool: &Pool,
    token_secret: &[u8; 32],
    metrics: &Arc<Metrics>,
    access: &SwappableAccess,
    delegations: Arc<crate::delegation::PgDelegationRegistry>,
    credentials: &Arc<dyn ordo_exec::route::DbCredentials>,
    space: &str,
) -> Result<(), BootRefusal> {
    let wiring = AccessRebuild {
        deployment,
        pool,
        token_secret,
        metrics,
        access,
        delegations,
        credentials,
    };
    rebuild_access(wiring, crate::directory::Scope::Space(space)).await
}

/// What a rebuild of the door needs.
struct AccessRebuild<'a> {
    deployment: &'a Deployment,
    pool: &'a Pool,
    token_secret: &'a [u8; 32],
    metrics: &'a Arc<Metrics>,
    access: &'a SwappableAccess,
    delegations: Arc<crate::delegation::PgDelegationRegistry>,
    credentials: &'a Arc<dyn ordo_exec::route::DbCredentials>,
}

/// One rebuild. The dials happen first, with no lock held. The cache is
/// then locked until the new door is installed, so two rebuilds install in
/// the order they built and the later one holds every newer read. The
/// program is read under the lock too, so a rebuild for a member change
/// cannot install the version a deploy just replaced.
async fn rebuild_access(
    w: AccessRebuild<'_>,
    scope: crate::directory::Scope<'_>,
) -> Result<(), BootRefusal> {
    let started = std::time::Instant::now();
    let template = w.pool.config();
    let reads = crate::directory::prefetch(w.pool, w.credentials.as_ref(), &template, scope).await;
    let mut cache = w.access.directory.lock().await;
    let live = w.deployment.current();
    let built = cache
        .build(
            reads,
            &live.fixture,
            w.pool,
            w.credentials.as_ref(),
            &template,
        )
        .await;
    let directory = match built {
        Ok(d) => d,
        Err(e) => {
            // The door serving now predates the change this rebuild was
            // for. Its space answers `server_busy` until one installs,
            // never `access_denied` (reported fix
            // tenant-access-lags-after-join).
            if let crate::directory::Scope::Space(space) = scope {
                w.access
                    .failed
                    .write()
                    .expect("door lock")
                    .insert(space.to_string());
            }
            return Err(BootRefusal::Engine(e));
        }
    };
    let (dialled, databases) = (directory.dialled, directory.databases);
    let kept = directory.kept_stale.clone();
    let (door, unread) = door_from(
        directory,
        w.pool,
        w.token_secret,
        w.metrics,
        w.access.recorder.clone(),
        w.delegations,
        w.access.revocations.clone(),
    );
    w.access.install(door);
    // Together with the door they describe, so the two can never be one
    // rebuild apart.
    w.access.note_unread(unread);
    *w.access.kept_stale.write().expect("door lock") = kept;
    w.access.failed.write().expect("door lock").clear();
    drop(cache);
    let ms = started.elapsed().as_millis();
    // A rebuild for one space is named, so a slow sign-up can be read off
    // the log. The tick's rebuild is named only when it was slow.
    if matches!(scope, crate::directory::Scope::Space(_)) || ms >= 1_000 {
        println!(
            "door: rebuilt over {databases} tenant database(s), {dialled} read again, in {ms} ms"
        );
    }
    Ok(())
}

// ── serving ──────────────────────────────────────────────────────────────

/// Bind, serve, and shut down in the order the tree's own data loss
/// requires.
///
/// ```text
/// 1  stop accepting            the listener closes
/// 2  drain in-flight HTTP      bounded by ORDO_SHUTDOWN_GRACE_SECS
/// 3  cancel the workers        R3
/// 4  flush the security drain  SecurityDrain::shutdown() -> Result<usize>
/// 5  release leadership        R3
/// 6  close the pool            drop; every socket closes
/// 7  exit 0 — or 1 if step 2 hit its bound
/// ```
pub async fn serve(mut rt: Runtime) -> Result<(), BootRefusal> {
    // 14 — the supervisor, BEFORE the door. A write that lands before
    // `drain_listener` has a cursor is a write whose events are read from
    // position 0 on the first drain: correct, but it makes the listener's
    // first pass walk the whole log. Registering cursors before traffic
    // keeps the first drain bounded.
    let supervisor = start_supervisor(&rt).await?;
    rt.platform = rt.platform.clone().with_supervisor(supervisor.clone());
    for (name, how) in supervisor.census() {
        println!("  worker      {name:<24} {how}");
    }
    for (name, why) in &supervisor.disabled {
        println!("  DISABLED    {name:<16} — {why}");
    }

    // P3: the census line for self-serve provisioning, read from the stored
    // setting rather than the environment because the stored one wins.
    if rt.platform.auth.is_some() {
        if let Some(line) = crate::self_serve::census_line(&rt.pool).await {
            println!("  provisioning {line}");
        }
    }

    // The address limits are proxy mode's. Set without it, each says what
    // turns it on, and the boot goes on.
    if rt.pacer.is_none() {
        for line in crate::proxy::inert_limits(|v| std::env::var_os(v).is_some()) {
            println!("  {line}");
        }
    }

    let app = door(&rt);

    // The signal handlers go in BEFORE the socket is bound. Tokio installs a
    // handler when the stream is created, and a SIGTERM that arrives while
    // none is installed takes the default action and kills the process
    // with no drain. Registering inside the shutdown future left exactly
    // that window between `bind` and the future's first poll, and an
    // orchestrator that probes the port and then stops the pod can land in
    // it (seen on CI, 2026-09-19).
    let shutdown = shutdown_signal();

    let listener = tokio::net::TcpListener::bind(rt.cfg_addr)
        .await
        .map_err(|e| BootRefusal::Serve(format!("cannot bind {}: {e}", rt.cfg_addr)))?;

    // G16: what started this deployment, in its own security record and on
    // `/health/deep` (`crate::boot_record`). Written once the port is held,
    // so a start that never served is not recorded as a boot. Hashing the
    // binary reads the whole file, so it runs beside serving rather than in
    // front of it.
    {
        let recorder = rt.access.recorder.clone();
        let at = now_secs();
        tokio::spawn(async move {
            let booted = crate::boot_record::BootRecord::of_this_process(at).await;
            println!("  boot        {}", booted.detail());
            crate::boot_record::install(booted, &recorder);
        });
    }

    // The deploy channel. Its refusal policy is already written and this
    // only has to obey it: a node that cannot load the new version must not
    // stop serving the old one, and the refusal is an operator alert, not
    // an outage.
    let deploys = spawn_deploy_listener(&rt).await;

    // This node's row in `$nodes`, from now until shutdown. The access
    // engine was built for the boot's version in `assemble`, so the door
    // serves it from here.
    rt.node_status
        .serving(&rt.deployment.current().version)
        .await;
    let heartbeat = rt.node_status.spawn_heartbeat();

    // When a sealed value last opened, per key epoch: loaded now so the
    // time from before this restart shows on `/health/deep`, and written at
    // most once per interval (never per read) and again at shutdown.
    let decrypt_secs: u64 = parsed(
        crate::decrypt_seen::FLUSH_SETTING,
        crate::decrypt_seen::DEFAULT_FLUSH_SECS,
    )?;
    if decrypt_secs == 0 {
        return Err(BootRefusal::Invalid {
            var: crate::decrypt_seen::FLUSH_SETTING,
            detail: "a zero-second interval writes on every tick".into(),
        });
    }
    let decrypts = rt.metrics.decrypts().clone();
    if let Err(e) = crate::decrypt_seen::DecryptLedger::ensure_table(&rt.pool).await {
        eprintln!("decrypt times: table not created ({e})");
    }
    if let Err(e) = decrypts.load(&rt.pool).await {
        eprintln!("decrypt times: not loaded ({e})");
    }
    let decrypt_flusher =
        decrypts.spawn_flusher(rt.pool.clone(), Duration::from_secs(decrypt_secs));

    // A process that stopped while it was making a person's own tenant left
    // its row `provisioning`. Each is made again now, on a task of its own:
    // every step of a provision can be run twice for one tenant (reported
    // fix slow-passkey-signup, owner 2026-10-21).
    if let Some(signup) = rt.platform.auth.as_ref().and_then(|a| a.signup()) {
        let resumed = signup.tenants.resume_own().await;
        if resumed > 0 {
            println!("  provisioning resumed for {resumed} tenant(s) left part-made");
        }
    }

    let grace = rt.shutdown_grace;
    // The grace bounds the **drain**, never the serving.
    //
    // This used to wrap the whole serve future in `timeout(grace + 1s)`,
    // which made the deployment stop of its own accord that long after it
    // bound, whatever traffic it was carrying, and report it as a drain
    // that overran. Every test here outlives a process by less than the
    // grace, so nothing caught it until the first cluster deployment sat
    // there and died on a timer (2026-09-20).
    //
    // So the signal is observed separately: the serve future runs
    // unbounded until it ends on its own or the signal fires, and only the
    // work after the signal is on a clock.
    let (signalled_tx, signalled) = tokio::sync::oneshot::channel::<()>();
    let shutdown = async move {
        shutdown.await;
        let _ = signalled_tx.send(());
    };
    // `IntoFuture`, not `Future`: the select below needs to hold it across
    // two branches.
    let served = std::future::IntoFuture::into_future(
        // With the peer's address: proxy mode reads it, and nothing else.
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown),
    );
    tokio::pin!(served);
    let outcome = tokio::select! {
        finished = &mut served => Ok(finished),
        _ = signalled => tokio::time::timeout(grace + Duration::from_secs(1), served).await,
    };

    if let Some(task) = deploys {
        task.abort();
    }
    // Gone from `$nodes`, so an apply does not wait for this node.
    heartbeat.abort();
    rt.node_status.leave().await;
    decrypt_flusher.abort();
    decrypts.flush(&rt.pool).await;

    let bounded_out = match outcome {
        Ok(Ok(())) => false,
        Ok(Err(e)) => return Err(BootRefusal::Serve(e.to_string())),
        Err(_) => {
            eprintln!(
                "shutdown: in-flight requests did not finish within {}s — \
                 terminationGracePeriodSeconds must exceed ORDO_SHUTDOWN_GRACE_SECS",
                grace.as_secs()
            );
            true
        }
    };

    // 3 — cancel the workers and run every final act, BEFORE the drain
    // flushes: M1's partial period belongs in the record the drain is about
    // to write. `shutdown` also releases leadership explicitly, which is
    // what lets the replacement replica take over in milliseconds rather
    // than at TCP timeout.
    //
    // Called through the `Arc`, on `&self`. This was
    // `if let Some(s) = Arc::into_inner(supervisor)`, and `PlatformState`
    // holds a clone of the same `Arc` so `/health/deep` can answer the
    // census, so `into_inner` returned `None` every time: the workers were
    // never cancelled, no final act ran, leadership was released only when
    // the process's socket closed, and the shipped binary printed nothing
    // about any of it and exited 0. `tests/runtime.rs` now asserts the
    // count and its position relative to the drain.
    let acts = supervisor.shutdown(Duration::from_secs(10)).await;
    println!("shutdown: {acts} worker final act(s) ran");

    // 4 — after the door has stopped and after the workers have. Flushing
    // before either has stopped flushes a moving target. The number is
    // logged because a bounded, known loss of audit record belongs in the
    // log rather than in nobody's memory.
    let Runtime { secdrain, .. } = rt;
    match secdrain.shutdown().await {
        Ok(n) => println!("shutdown: security drain flushed {n} entries"),
        Err(e) => eprintln!("shutdown: security drain did not flush cleanly: {e}"),
    }

    if bounded_out {
        Err(BootRefusal::Serve("shutdown exceeded its grace".into()))
    } else {
        Ok(())
    }
}

/// The whole HTTP surface [`serve`] binds, wrapped in the CORS allowlist
/// when `ORDO_CORS_ORIGINS` is set: the op door, the blob routes when
/// a store is configured, and the platform routes over `rt.platform`.
///
/// Public so a test can bind exactly what the binary binds, with a
/// supervisor of its own attached through
/// [`crate::serve::PlatformState::with_supervisor`].
///
/// `router_with_blobs` when a store is configured, `router` when there is
/// none. Two builders rather than one that takes an `Option`, because the
/// body limits differ: the op route is capped at a JSON envelope's size and
/// the upload route at the object cap, and a single `.layer()` over the
/// merged tree would have to choose one of them for both.
///
/// A deployment with no store reaches this only for a program that declares
/// no stored blob field (`assemble` refuses otherwise), so an absent blob
/// route is a program with no files, not a door with a hole.
pub fn door(rt: &Runtime) -> axum::Router {
    let ops = ordo_api::router(rt.api.clone());
    let app = match rt.blobs.clone() {
        Some(blobs) => ordo_api::router_with_blobs(rt.api.clone(), blobs),
        None => ops,
    }
    // `GET /download`: a person's own data as one zip (`ordo_api::download`).
    // Files come through the blob port when a store is configured.
    .merge(ordo_api::download::router(
        rt.api.clone(),
        rt.blobs.clone(),
        rt.download.clone(),
    ))
    .merge(crate::serve::platform_router(rt.platform.clone()))
    // `POST /fn/<name>`: backend functions, through the same door.
    .merge(ordo_api::function_routes(
        rt.api.clone(),
        rt.functions.clone(),
    ));
    // Proxy mode's pacer, inside the CORS layer so a preflight is never
    // slowed. `None` changes nothing.
    let app = crate::proxy::apply(app, rt.pacer.clone());
    // Outermost, so the preflight is answered before routing and every
    // route's response carries the same headers. `None` changes nothing.
    ordo_api::cors::apply_layers(
        app,
        ordo_api::cors::Cors {
            deployment: rt.cors.clone(),
            tenants: (rt.tenant_origins.max() > 0)
                .then(|| rt.tenant_origins.clone() as Arc<dyn ordo_api::cors::TenantOrigins>),
            public_ops: Some(rt.api.registry.clone() as Arc<dyn ordo_api::cors::PublicOps>),
        },
    )
}

/// SIGTERM and SIGINT. Orchestrators send the first; a terminal sends the
/// second.
///
/// A plain function returning the future, so that both handlers are
/// installed when it is called rather than when the future is first
/// polled. See the call site for why that matters.
fn shutdown_signal() -> impl std::future::Future<Output = ()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let term = signal(SignalKind::terminate()).ok();
        let int = signal(SignalKind::interrupt()).ok();
        async move {
            let (Some(mut term), Some(mut int)) = (term, int) else {
                return std::future::pending().await;
            };
            tokio::select! {
                _ = term.recv() => println!("shutdown: SIGTERM"),
                _ = int.recv() => println!("shutdown: SIGINT"),
            }
        }
    }
    #[cfg(not(unix))]
    {
        async move {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

/// Listen for deploy notifications and swap what they name.
///
/// Returns `None` when the channel cannot be opened, which is logged and is
/// not fatal: a node that cannot hear about deploys still serves the
/// version it has.
async fn spawn_deploy_listener(rt: &Runtime) -> Option<tokio::task::JoinHandle<()>> {
    let (listener, mut rx) = match deploy::listen_for_changes(&rt.pg).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!(
                "deploy channel: not listening ({e}) — deploys and new tenants need a restart"
            );
            return None;
        }
    };
    let pool = rt.pool.clone();
    let pg = rt.pg.clone();
    let keys = rt.keys.clone();
    let space = rt.space.clone();
    let deployment = rt.deployment.clone();
    let access = rt.access.clone();
    let metrics = rt.metrics.clone();
    let token_secret = rt.token_secret;
    let has_store = rt.blobs.is_some();
    let delegations = rt.delegations.clone();
    let credentials = rt.credentials.clone();
    let deployment_owners = rt.deployment_owners.clone();
    let pg_bus = rt.pg_bus.clone();
    let public = rt.api.public.clone();
    let catchup = rt.catchup.clone();
    let node_status = rt.node_status.clone();
    let routines = rt.routines.clone();
    let spaces = rt.spaces.clone();
    let functions = rt.functions.clone();
    // The serving version's functions, read before the first call can come.
    {
        let current = deployment.current();
        match functions
            .reload(&pool, &current.version, &current.fixture.schema)
            .await
        {
            Ok(0) => {}
            Ok(n) => println!("functions: {n} loaded for version {}", current.version),
            Err(e) => eprintln!(
                "functions: none served for version {}: {e}",
                current.version
            ),
        }
    }
    // The boot's sweep: a tenant whose database was left behind while this
    // node was down is brought forward now rather than when it is asked for.
    catchup.nudge();
    Some(tokio::spawn(async move {
        // Held for the task's life: dropping the listener closes the
        // channel.
        let _listener = listener;
        while let Some(note) = rx.recv().await {
            // A new tenant, not a new version. The door is rebuilt and
            // nothing else is: no artifact is loaded, no version changes,
            // and `Deployment::swap` is not called, because the program
            // this node serves has not moved. See
            // `ordo_exec::jobs::MEMBERS_CHANNEL` for why this is its own
            // channel rather than a deploy with an empty payload.
            if note.channel == ordo_exec::jobs::MEMBERS_CHANNEL {
                let space = note.payload.trim();
                // A pause or a resume on another replica is announced here
                // too (27ar). The register is reloaded whole from the
                // registry, so a resume lifts as surely as a pause holds.
                if let Err(e) = crate::pause::reload(&spaces, &pool).await {
                    eprintln!("space {space}: pause register not reloaded ({e}); the refresh tick tries again");
                }
                // Listen to the new tenant's database before the door
                // serves it, so no write to it is ever unheard. A failure
                // is a log line: the access refresh tick tries again.
                match pg_bus.refresh().await {
                    Ok(0) => {}
                    Ok(n) => println!("space {space}: now heard on {n} more database(s)"),
                    Err(e) => eprintln!("space {space}: live updates not attached yet ({e})"),
                }
                match refresh_access_for(
                    &deployment,
                    &pool,
                    &token_secret,
                    &metrics,
                    &access,
                    delegations.clone(),
                    &credentials,
                    space,
                )
                .await
                {
                    // Named at info level rather than kept quiet: an
                    // operator watching a tenant being created should see
                    // the moment it became reachable on this replica,
                    // because the alternative explanation for a 404 a
                    // second later is a bug.
                    Ok(()) => println!("space {space}: door rebuilt, now reachable on this node"),
                    // The tenant exists and this node cannot reach it.
                    // Not fatal and not retried here: a node that stops
                    // serving because one rebuild failed takes every
                    // existing tenant down over a tenant that has no rows
                    // yet. The next deploy rebuilds the door anyway, and a
                    // restart certainly does.
                    Err(e) => eprintln!(
                        "space {space}: created, and this node could not rebuild its door \
                         ({e}) — it is reachable on this node after the next deploy or restart"
                    ),
                }
                continue;
            }
            let version = note.payload.trim().to_string();
            let loaded =
                deploy::load_version(&pool, &pg, &version, keys.clone(), space.clone()).await;
            let verified = match loaded {
                Ok(v) => v,
                Err(e) => {
                    // The refusal is an operator alert, not an outage.
                    metrics.incr(crate::metrics::SHAPE_FAILURES, &[]);
                    eprintln!("deploy {version}: refused, still serving the old version — {e}");
                    continue;
                }
            };
            // The swap carries a freshly compiled fixture, so the custody
            // assertion runs again. A boot-time-only check would not catch
            // a regression that reached the tree after the first load.
            if !verified.custody_is(&keys) {
                eprintln!(
                    "deploy {version}: refused — the candidate does not seal through this \
                     deployment's KeyProvider"
                );
                continue;
            }
            // The same rule the boot took, on the same terms: a version
            // that declares a stored blob field is a version this node
            // cannot serve without a store. Refused rather than installed,
            // so the node keeps serving the old one — the deploy channel's
            // stated policy, and the alternative here is a door that
            // publishes a file field and 500s on the first upload.
            if !has_store {
                if let Some(field) = blob_needed(verified.schema()) {
                    eprintln!(
                        "deploy {version}: refused — `{field}` is a stored blob and \
                         ORDO_BLOB_STORE is not set on this node"
                    );
                    continue;
                }
            }
            match deployment.swap(verified) {
                Swapped::AlreadyServing(v) => println!("deploy {v}: already serving"),
                Swapped::Refused(why) => {
                    metrics.incr(crate::metrics::SHAPE_FAILURES, &[]);
                    eprintln!("deploy {version}: refused, still serving the old version — {why}");
                }
                Swapped::Installed { from, to } => {
                    println!("deploy: {from} -> {to}");
                    // New calls run the new version's code; calls in flight
                    // finish on the entry they started with.
                    let schema = deployment.current().fixture.schema.clone();
                    if let Err(e) = functions.reload(&pool, &to, &schema).await {
                        eprintln!("deploy {to}: no functions served: {e}");
                    }
                    // The bus learns the new version's ops and entities, and
                    // the public cache drops everything: a deploy may move
                    // rows, and one rule is easier to trust than a per-op
                    // diff (`design-public-path.md` §3, ruled ask 11).
                    crate::public::after_deploy(&pg_bus, &public, &deployment.current().fixture);
                    // Every tenant's database that has not followed is
                    // closed as behind until it does. Asked for here, on
                    // every replica, because an apply made straight against
                    // the deployment's database reaches no tenant's.
                    catchup.nudge();
                    // 17v: the first `ordo apply` is what creates `members`,
                    // so it is also the moment the system space's founding
                    // row becomes writable on a deployment that booted
                    // against an empty database. Before the door rebuild
                    // below, which reads that table, so the row lands in the
                    // engine this swap installs rather than at the next
                    // restart. Idempotent on every later deploy, and a
                    // failure is a log line: a node that is serving must not
                    // stop serving over it.
                    match crate::provision::ensure_system_space(&pool, keys.as_ref()).await {
                        Ok(crate::provision::SystemSpace::Created) => {
                            println!(
                                "system space {}: founding row written",
                                crate::provision::SYSTEM_SPACE
                            )
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!(
                            "system space {}: not completed ({e}); it is minted but has no \
                             member row",
                            crate::provision::SYSTEM_SPACE
                        ),
                    }
                    // P0: the same first `apply` is when the deployment
                    // owners the environment names can be written, on a
                    // deployment that booted against an empty database.
                    match crate::provision::ensure_deployment_owners(
                        &pool,
                        keys.as_ref(),
                        &space.0,
                        &deployment_owners,
                        Some(access.recorder.as_ref()),
                        now_secs(),
                    )
                    .await
                    {
                        Ok(done) => report_deployment_owners(done, &space.0),
                        Err(e) => eprintln!("deployment owners: not completed ({e})"),
                    }
                    // A swap changes the ops, and the engine pins a
                    // manifest per op. Rebuilding is what keeps a
                    // capability grant tied to what it consented to.
                    match refresh_access(
                        &deployment,
                        &pool,
                        &token_secret,
                        &metrics,
                        &access,
                        delegations.clone(),
                        &credentials,
                    )
                    .await
                    {
                        Ok(()) => {}
                        Err(e) => eprintln!(
                            "deploy {to}: the access engine did not rebuild ({e}) — \
                             the door is still pinning the previous version's manifests"
                        ),
                    }
                    // The new version's routines, with no restart: added
                    // ones start, removed ones stop, retimed ones move.
                    // Before the row below, so an apply that waited sees
                    // them running.
                    match routines
                        .switch(&pool, &deployment.current().fixture.schema, &to)
                        .await
                    {
                        Ok(line) => println!("{line}"),
                        Err(e) => eprintln!(
                            "deploy {to}: the routines were not switched ({e}); the previous \
                             version's keep running until a restart"
                        ),
                    }
                    // The door serves `to` from here: said in `$nodes`, which
                    // is what `ordo apply` waits on.
                    node_status.serving(&to).await;
                }
            }
        }
        eprintln!("deploy channel: closed — deploys now need a restart");
    }))
}

/// Build the worker set this deployment can actually run, and start it.
///
/// **Enabling conditions are the census.** Every worker whose condition
/// does not hold gets a `DISABLED` line naming the variable or the missing
/// piece — obligation 8 — because a feature that is off and unmentioned is
/// a feature an operator believes is on.
async fn start_supervisor(rt: &Runtime) -> Result<Arc<crate::supervisor::Supervisor>, BootRefusal> {
    use crate::supervisor::{Plan, Worker};
    use crate::workers::*;

    // Before anything registers a cursor: `register_listener` writes a
    // `$logconsumers` row, and `ensure_platform_tables` deliberately does
    // not create that table.
    crate::supervisor::bootstrap(&rt.pool)
        .await
        .map_err(|e| BootRefusal::Database(e.to_string()))?;

    let mut workers: Vec<Arc<dyn Worker>> = Vec::new();
    let mut disabled: Vec<(String, String)> = Vec::new();

    let listeners = Arc::new(rt.triggers.listeners.clone());
    let clock = Arc::new(rt.triggers.clock.clone());

    // 11-15: time out a provision that stopped part way, so a stuck tenant
    // becomes visible in `tenants::troubled` rather than sitting in
    // `provisioning` until somebody goes looking. It only marks; releasing
    // the resources is `std.space.reap`, which a person calls.
    //
    // Unconditional, and **outside** the listener branch below, which is
    // where it used to be: every deployment creates tenants, no deployment
    // declares a listener until 12a lands, so registering it beside the
    // trigger workers meant it never ran anywhere and the census did not
    // name it as not running either. Tenant reconciliation has nothing to
    // do with trigger listeners.
    workers.push(Arc::new(crate::workers::ProvisioningReconciler::from_env()));

    // Owner, 2026-10-21: every running server sweeps for stalled setups of
    // a person's own tenant and keeps trying them, under a lease so two
    // servers never run one. It also reports a stuck one.
    if let Some(signup) = rt.platform.auth.as_ref().and_then(|a| a.signup()) {
        workers.push(Arc::new(crate::tenant_setup::TenantSetupSweep {
            ops: signup.tenants.clone(),
        }));
    }

    // 25i: a tenant's database follows the program. Every replica runs it,
    // because every replica holds its own register of closed tenants and
    // lifts its own; the DDL itself is one replica's, under the apply lock.
    workers.push(Arc::new(crate::tenant_forward::TenantCatchUp {
        catchup: rt.catchup.clone(),
        every: crate::tenant_forward::TenantCatchUp::period_from_env(),
    }));

    // S6-004: who may reach what is re-read on a timer, so a member who is
    // removed or suspended stops being served within one period and no
    // deployment is restarted over it. Unconditional for the reconciler's
    // reason: every deployment has members.
    workers.push(Arc::new(crate::workers::AccessRefresh {
        access: rt.access.clone(),
        token_secret: rt.token_secret,
        delegations: rt.delegations.clone(),
        credentials: rt.credentials.clone(),
        bus: Some(rt.pg_bus.clone()),
        every: crate::workers::AccessRefresh::period_from_env(),
    }));

    // A second replica learns of a logout, and of every session ended, by
    // re-reading the deny list. The list answers from memory, so without
    // this it changes only on restart. Present when authentication is: that
    // is when there is a list to read.
    if let Some(store) = rt
        .platform
        .auth
        .as_ref()
        .and_then(|a| a.session_store.clone())
    {
        workers.push(Arc::new(crate::workers::SessionsRefresh {
            store,
            renewals: rt
                .platform
                .auth
                .as_ref()
                .and_then(|a| a.remember.as_ref())
                .map(|r| r.store.clone()),
            policies: rt.platform.auth.as_ref().and_then(|a| a.policy.clone()),
            every: crate::workers::SessionsRefresh::period_from_env(),
        }));
    } else {
        disabled.push((
            "sessions.refresh".into(),
            "no authentication method is configured, so there is no deny list to re-read".into(),
        ));
    }

    // A second replica learns of a level a tenant's owner set by re-reading
    // it. The door answers from memory, so without this it changes only on
    // restart.
    // 27ar: every replica reloads its pause register on the access refresh
    // period, the ceiling for a missed announcement.
    workers.push(Arc::new(crate::workers::PauseRefresh {
        spaces: rt.spaces.clone(),
        every: crate::workers::AccessRefresh::period_from_env(),
    }));

    workers.push(Arc::new(crate::workers::SecondStepLevelsRefresh {
        levels: rt.second_levels.clone(),
        every: crate::workers::AccessRefresh::period_from_env(),
    }));

    // A tenant's own CORS origins, set on another replica, reach this one
    // on the same period.
    if rt.tenant_origins.max() > 0 {
        workers.push(Arc::new(crate::workers::TenantOriginsRefresh {
            origins: rt.tenant_origins.clone(),
            every: crate::workers::AccessRefresh::period_from_env(),
        }));
    }

    // The mail outbox, drained. Present when the app developer configured a
    // channel (`plan-auth-federated.md` §7.6); off by default.
    match &rt.mail {
        Some(channel) => workers.push(Arc::new(crate::mail::MailSend {
            mailer: channel.mailer.clone(),
            composers: Arc::new(crate::mail::boot_composers(
                rt.platform.auth.as_ref().and_then(|a| {
                    a.email
                        .clone()
                        .or_else(|| a.contact().and_then(|c| c.codes().cloned()))
                }),
            )),
            policy: channel.policy.clone(),
        })),
        None => disabled.push((
            crate::mail::WORKER_NAME.into(),
            "ORDO_MAIL_KIND is not set, so the deployment sends no mail".into(),
        )),
    }

    // The open-ceremony table, swept of what expired. Present when
    // authentication is: that is when the table has a writer.
    match rt.platform.auth.as_ref().and_then(|a| a.ceremonies.clone()) {
        Some(store) => workers.push(Arc::new(crate::workers::CeremonySweep {
            store,
            email: rt
                .platform
                .auth
                .as_ref()
                .and_then(|a| {
                    a.email
                        .clone()
                        .or_else(|| a.contact().and_then(|c| c.codes().cloned()))
                })
                .map(|e| e.store()),
            every: Duration::from_secs(crate::workers::CeremonySweep::DEFAULT_SECS),
        })),
        None => disabled.push((
            "auth.ceremony.sweep".into(),
            "no authentication method is configured, so no ceremony is ever open".into(),
        )),
    }

    // The end of a recovery's contest window: the passkeys it retired are
    // removed, their public keys with them. Present when recovery is.
    match rt
        .platform
        .auth
        .as_ref()
        .filter(|a| a.recovery().is_some() && a.passkey.is_some())
    {
        Some(auth) => workers.push(Arc::new(crate::workers::RecoverySweep {
            auth: auth.clone(),
            every: Duration::from_secs(crate::workers::RecoverySweep::DEFAULT_SECS),
        })),
        None => disabled.push((
            "auth.recovery.sweep".into(),
            "no passkey authentication is configured, so no recovery retires a passkey".into(),
        )),
    }

    // 2026-10-06: a deleted account's data, removed when the app developer
    // said so. The worker follows the setting in force (ask 12): under
    // `<n>d` it removes accounts past `deleted_at + n days`; under `now` its
    // boot pass removes everyone still waiting (the request removes at
    // once); under `never` nothing is ever due, so it is not started.
    match crate::account_removal::Removal::from_env() {
        Ok(
            removal @ (crate::account_removal::Removal::AfterDays(_)
            | crate::account_removal::Removal::Now),
        ) => {
            workers.push(Arc::new(crate::account_removal::AccountRemovalSweep {
                stream: rt.access.recorder.clone(),
                every: Duration::from_secs(
                    crate::account_removal::AccountRemovalSweep::DEFAULT_SECS,
                ),
                removal,
            }));
        }
        Ok(_) => disabled.push((
            crate::account_removal::AccountRemovalSweep::NAME.into(),
            "ORDO_ACCOUNT_REMOVAL is `never`, so no deleted account is removed".into(),
        )),
        Err(detail) => {
            return Err(BootRefusal::Invalid {
                var: crate::account_removal::REMOVAL_ENV,
                detail,
            })
        }
    }

    // E4: the sweep of registrations no tenant admitted. On by default, at
    // 90 days (owner, 2026-09-29); the variable overrides the lifetime and
    // the census names the value in use.
    match crate::registrations::ttl_from_env() {
        // The sweep reads `$principal`, which boot creates only when an
        // authentication method is configured. Without one there is no
        // directory to sweep, the same condition `auth.recovery.sweep` and
        // `auth.ceremony.sweep` wait on.
        Ok(_) if rt.platform.auth.is_none() => disabled.push((
            crate::registrations::RegistrationSweep::NAME.into(),
            "no authentication method is configured, so there is no identity directory to sweep"
                .into(),
        )),
        Ok(ttl_secs) => {
            println!(
                "  registrations {}",
                crate::registrations::describe_ttl(ttl_secs)
            );
            // A guest's own lifetime, when the deployment set one. Read
            // again here; `auth_from_env` already refused a bad value.
            let guest_ttl_secs =
                crate::auth::guest::GuestConfig::from_vars(&|k: &str| std::env::var(k).ok())
                    .ok()
                    .and_then(|g| g.ttl_secs);
            if let Some(g) = guest_ttl_secs {
                println!("  registrations: a guest that claimed nothing is kept {g}s");
            }
            let every = crate::registrations::RegistrationSweep::period_for(
                guest_ttl_secs.map_or(ttl_secs, |g| g.min(ttl_secs)),
            );
            workers.push(Arc::new(crate::registrations::RegistrationSweep {
                pools: rt.spaces.pools().clone(),
                stream: rt.access.recorder.clone(),
                ttl_secs,
                guest_ttl_secs,
                every,
            }));
        }
        Err(detail) => {
            return Err(BootRefusal::Invalid {
                var: "ORDO_AUTH_REGISTRATION_TTL_SECS",
                detail,
            })
        }
    }

    // Backend functions: runs and their logs are kept
    // `ORDO_FN_RUN_RETENTION_DAYS` days, 14 by default (plan §10). Always
    // started: a version switch may add a function with no restart, and a
    // pass over a deployment with no `$fnruns` does nothing.
    match crate::fn_runs::retention_from_env() {
        Ok(keep_days) => workers.push(Arc::new(crate::fn_runs::FnRunSweep {
            keep_days,
            every: Duration::from_secs(crate::fn_runs::FnRunSweep::DEFAULT_SECS),
        })),
        Err(detail) => {
            return Err(BootRefusal::Invalid {
                var: crate::fn_runs::RETENTION_ENV,
                detail,
            })
        }
    }

    // A deployment whose routines follow the program runs every trigger
    // worker, idle while the active version declares none, so a version
    // switch that adds a routine starts it with no restart.
    let follows = rt.routines.follows_program();
    if listeners.is_empty() && !follows {
        for w in ["triggers.drain", "triggers.batch"] {
            disabled.push((
                w.into(),
                "the program declares no routine woken by a change".into(),
            ));
        }
    } else {
        // The cursor first: `register_listener` pins the pruning floor, and
        // a listener that drained before it registered would be a listener
        // whose at-least-once guarantee starts at whatever the floor
        // happened to be.
        let now = now_secs();
        match &rt.triggers.tenants {
            // The program's routines: every tenant known now gets a cursor
            // at the head of its chain, before the door opens.
            Some(tenants) => {
                for space in tenants.list(&rt.pool).await {
                    let pool = match tenants.spaces.pool_of(&space).await {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("triggers: tenant {space} not reached at boot: {e}");
                            continue;
                        }
                    };
                    tenants
                        .register(&pool, &space, &listeners, true, now)
                        .await
                        .map_err(|e| BootRefusal::Database(e.to_string()))?;
                }
            }
            None => {
                for l in listeners.iter() {
                    ordo_exec::triggers::register_listener(
                        &rt.pool,
                        &ordo_commandlog::SpaceId(rt.space.0.clone()),
                        l,
                        0,
                        now,
                    )
                    .await
                    .map_err(|e| BootRefusal::Database(e.to_string()))?;
                }
            }
        }
        workers.push(Arc::new(TriggersDrain {
            live: rt.routines.clone(),
            store: ordo_exec::commandlog::PgStore::new(
                rt.pool.clone(),
                tokio::runtime::Handle::current(),
            ),
        }));
        workers.push(Arc::new(TriggersBatch {
            live: rt.routines.clone(),
        }));
    }

    match &rt.triggers.handler {
        Some(_) => {
            crate::routines::ensure_tables(&rt.pool)
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
            workers.push(Arc::new(TriggersRunner {
                live: rt.routines.clone(),
            }))
        }
        None if follows => workers.push(Arc::new(TriggersRunner {
            live: rt.routines.clone(),
        })),
        None => disabled.push((
            "triggers.runner".into(),
            "the program declares no routine (checklist 12a)".into(),
        )),
    }

    if clock.is_empty() && !follows {
        disabled.push((
            "triggers.clock".into(),
            "the program declares no clock routine".into(),
        ));
    } else {
        if !clock.is_empty() {
            ordo_exec::triggers::register_clock(&rt.pool, &clock, now_secs())
                .await
                .map_err(|e| BootRefusal::Database(e.to_string()))?;
        }
        workers.push(Arc::new(TriggersClock {
            live: rt.routines.clone(),
        }));
    }

    match &rt.checkpoint_signer {
        Some(signer) => {
            workers.push(Arc::new(CommandlogCheckpoint {
                signer: signer.clone(),
                policy: rt.checkpoint_policy,
                witness: rt.witness.clone(),
            }));
            workers.push(Arc::new(CommandlogAttest {
                signer: signer.clone(),
                policy: rt.checkpoint_policy,
            }));
            // The delivery loop for what those two publish. Not started as
            // its own `AttestDelivery` task: two loops over one outbox
            // would claim against each other.
            match crate::attestdrain::DeliveryConfig::from_env(ordo_commandlog::SpaceId(
                rt.space.0.clone(),
            )) {
                Ok(cfg) => {
                    ordo_exec::webhook::ensure_tables(&rt.pool)
                        .await
                        .map_err(|e| BootRefusal::Database(e.to_string()))?;
                    workers.push(Arc::new(AttestDeliver {
                        cfg,
                        health: Arc::new(crate::attestdrain::DeliveryHealth::default()),
                    }));
                }
                // A malformed `ORDO_EGRESS_DENY_CIDRS` is an error, not a
                // warning: a silently-dropped pod CIDR lets a sink reach
                // the cluster's own service network.
                Err(e) => {
                    return Err(BootRefusal::Invalid {
                        var: "ORDO_EGRESS_DENY_CIDRS",
                        detail: e,
                    })
                }
            }
        }
        None => {
            for w in [
                "commandlog.checkpoint",
                "commandlog.attest",
                "attest.deliver",
            ] {
                disabled.push((w.into(), "ORDO_CHECKPOINT_KEY is not set".into()));
            }
        }
    }

    // The security stream's **checkpoint** cadence, on its own schedule row.
    // This is what makes `ordo verify`'s fourth check — nothing was removed
    // from the end of the stream — reachable in a running deployment rather
    // than only by an operator building a marker by hand. Registered
    // independently of retention below, which is the whole point of the
    // separation: see `workers::SecEventCheckpoint`.
    match (rt.security_checkpoint, rt.secdrain.checkpointer()) {
        (true, Some(checkpointer)) => workers.push(Arc::new(SecEventCheckpoint {
            checkpointer,
            tick: Duration::from_secs(60),
        })),
        (true, None) => disabled.push((
            "secevent.checkpoint".into(),
            rt.secdrain
                .health()
                .checkpoint_note()
                .unwrap_or_else(|| "the security drain holds no checkpoint signer".into()),
        )),
        (false, _) => disabled.push((
            "secevent.checkpoint".into(),
            "there is no security checkpoint key (ORDO_SECURITY_CHECKPOINT_KEY, see the boot \
             log), so nothing signs the head of any \
             security stream and entries removed from the END of one are undetectable"
                .into(),
        )),
    }

    // Security-stream retention, on its own schedule row and only with the
    // key a lawful cut point is signed with. The drain was started with
    // `supervised_retention`, so this worker is the only thing that runs the
    // pass; a key that is set and does not parse leaves the drain with no
    // retainer, and the drain's own note says why.
    match (rt.security_checkpoint, rt.secdrain.retainer()) {
        (true, Some(retainer)) => workers.push(Arc::new(SecEventRetain {
            retainer,
            tick: Duration::from_secs(60),
        })),
        (true, None) => disabled.push((
            "secevent.retain".into(),
            rt.secdrain
                .health()
                .retention_note()
                .unwrap_or_else(|| "the security drain holds no retention signer".into()),
        )),
        (false, _) => disabled.push((
            "secevent.retain".into(),
            "there is no security checkpoint key: ORDO_SECURITY_CHECKPOINT_KEY does not parse, \
             or the one made at first boot could not be opened (see the boot log)"
                .into(),
        )),
    }

    // `secevent.drain` is in the census and is not a supervised worker.
    //
    // §3.4 asks for it to be wrapped in the `Worker` shape, and it is
    // reported here instead, because adopting it would change three things
    // the drain guarantees and the plan says R3 must not change:
    //
    // - **It starts before the door and outlives the supervisor.** The
    //   drain starts in `assemble`, so a denial recorded by the first
    //   request is already headed for the database, and it flushes at §1.5
    //   step 4, after the workers have stopped. A supervised worker starts
    //   in `serve` and stops at step 3.
    // - **Its final flush is fallible and reported.** `shutdown()` returns
    //   what it wrote or the fault; `on_shutdown` runs inside the
    //   supervisor's grace and its error is a log line.
    // - **It must never be quarantined.** A supervised worker stops after
    //   ten consecutive failures. The drain failing ten times is a database
    //   outage, and stopping it then would turn a bounded, declared loss
    //   (the outbox bound, `EventsDropped`) into an unbounded silent one.
    //   The drain already has its own answer to a failing store:
    //   `failing_since` and `degraded` on `/health/deep`.
    //
    // So it appears as an `External`: in the census with `supervised:
    // false`, beside its own counters.
    let drain_health = rt.secdrain.health();
    let external = vec![crate::supervisor::External {
        name: "secevent.drain",
        health: Arc::new(move || {
            serde_json::json!({
                "appended": drain_health.appended(),
                "failures": drain_health.failures(),
                "pruned": drain_health.pruned(),
            })
        }),
    }];

    // Pruning is the one worker whose failure mode is destructive rather
    // than delayed, so it is one deliberate variable and it needs the
    // verifying key: a prune verifies the cut point and the prefix it is
    // about to destroy.
    let prune_on = matches!(
        std::env::var("ORDO_WORKER_PRUNE").as_deref(),
        Ok("1" | "true" | "yes")
    );
    match (prune_on, prune_verifier()) {
        (true, Some(verifier)) => workers.push(Arc::new(CommandlogPrune {
            verifier,
            policy: rt.checkpoint_policy,
            period: Duration::from_secs(3600),
        })),
        (true, None) => disabled.push((
            "commandlog.prune".into(),
            "ORDO_WORKER_PRUNE is set but ORDO_CHECKPOINT_KEY_ID / \
             ORDO_CHECKPOINT_PUBKEY are not — a prune verifies what it destroys"
                .into(),
        )),
        (false, _) => disabled.push((
            "commandlog.prune".into(),
            "ORDO_WORKER_PRUNE is not set (pruning stays an operator act)".into(),
        )),
    }

    // K1: the re-seal worker, always registered. It idles on one indexed
    // read per tick until `ordo keys rotate` or `ordo keys reseal` enqueues
    // a pass, and it records completion in the space's own security
    // stream, which is the recorder the door already drains into.
    workers.push(Arc::new(KeysReseal::from_env(
        Some(rt.access.recorder.clone()),
        rt.stores.clone(),
    )));

    // 18p U8: the storage sweep, on the one deployment-wide cadence the
    // owner ruled (D-S3, `ORDO_BLOB_GC_SECS`). It walks every database
    // `rt.spaces` can reach and resolves each space's store through the
    // same resolver the door writes through. Present when a store is: with
    // no `ORDO_BLOB_STORE` there is nothing to collect from, and the census
    // names the variable rather than leaving the worker out.
    match &rt.stores {
        Some(stores) => workers.push(Arc::new(BlobGc {
            spaces: rt.spaces.clone(),
            stores: stores.clone(),
            policy: rt.checkpoint_policy,
            period: BlobGc::period_from_env(),
            stream: Some(rt.access.recorder.clone()),
        })),
        None => disabled.push((
            "blob.gc".into(),
            "ORDO_BLOB_STORE is not set, so there is no store to collect from".into(),
        )),
    }

    // 17n M1b: `usage.flush` runs on every replica, gated on the ledger
    // itself rather than on a variable of its own — a deployment that
    // never turned on `ORDO_USAGE_LEDGER` has nothing to flush, and a
    // second variable to keep in step with the first would just be a way
    // for the two to disagree.
    // 17n M1c: `usage.rollup` runs on the leader only, over every database
    // `rt.spaces` can reach. Same gate as `usage.flush` above, for the same
    // reason — there is nothing for either to do with the ledger off.
    if rt.metrics.usage_enabled() {
        workers.push(Arc::new(UsageFlush {
            metrics: rt.metrics.clone(),
            replica: rt.replica.clone(),
            period: rt.usage_flush,
            quotas: rt.quotas.clone(),
        }));
        workers.push(Arc::new(UsageRollup {
            spaces: rt.spaces.clone(),
            period: rt.usage_rollup,
        }));
    } else {
        disabled.push(("usage.flush".into(), "ORDO_USAGE_LEDGER is not set".into()));
        disabled.push(("usage.rollup".into(), "ORDO_USAGE_LEDGER is not set".into()));
    }

    // `ORDO_WORKERS` is the role switch `implementation.md`'s "one binary,
    // role by config" names: a worker pod runs the same image with a subset
    // and no traffic routed to it.
    let allow = std::env::var("ORDO_WORKERS").ok().map(|s| {
        s.split(',')
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
    });

    // §3.4's per-worker tick overrides. Read here beside `ORDO_WORKERS` and
    // `ORDO_WORKER_PRUNE`, and refused rather than defaulted when
    // unreadable: a variable an operator set and the process ignored is
    // worse than one it never read.
    let ticks =
        crate::supervisor::tick_overrides(workers.iter().map(|w| w.name())).map_err(|detail| {
            BootRefusal::Invalid {
                var: "ORDO_WORKER_<NAME>_TICK_MS",
                detail,
            }
        })?;

    crate::supervisor::Supervisor::start(
        rt.pool.clone(),
        rt.keys.clone(),
        rt.deployment.clone(),
        rt.metrics.clone(),
        rt.space.clone(),
        rt.pg.clone(),
        Plan {
            workers,
            allow,
            disabled,
            external,
            listeners: listeners.iter().map(|l| l.key()).collect(),
            timing: crate::supervisor::Timing::default(),
            ticks,
        },
    )
    .await
    .map(Arc::new)
    .map_err(BootRefusal::Database)
}

/// The verifying key the prune worker checks its cut point with.
fn prune_verifier() -> Option<ordo_commandlog::CheckpointVerifier> {
    let id = std::env::var("ORDO_CHECKPOINT_KEY_ID").ok()?;
    let hex = std::env::var("ORDO_CHECKPOINT_PUBKEY").ok()?;
    ordo_commandlog::CheckpointVerifier::from_public_hex(id, &hex).ok()
}

impl Runtime {
    /// Register the listeners, clock routines and run handler this
    /// deployment serves.
    ///
    /// Called before [`serve`], by a test or an embedder that wants other
    /// wiring than the program's. The binary does not call it: assembly
    /// already wired the program's routines (checklist 12a), and a program
    /// with none leaves `triggers.drain`, `triggers.batch` and
    /// `triggers.runner` disabled with the reason.
    /// A wiring set here is fixed: a version switch does not replace it.
    pub fn with_triggers(mut self, wiring: TriggerWiring) -> Runtime {
        self.routines = Arc::new(crate::routines::LiveRoutines::fixed(
            wiring.clone(),
            self.spaces.clone(),
        ));
        self.triggers = wiring;
        self
    }

    /// Start the workers without binding a socket.
    ///
    /// [`serve`] does this itself; this exists for the two callers that
    /// want the loops and not the door — `ordo dev`, and a test that
    /// registers a `RunHandler` (which is a Rust value, so there is no wire
    /// to register one over — which is exactly what makes it a seam).
    pub async fn start_workers(&self) -> Result<Arc<crate::supervisor::Supervisor>, BootRefusal> {
        start_supervisor(self).await
    }
}

#[cfg(test)]
mod refine_limits {
    use super::*;

    /// The boot refuses a refinement limit of 0 or a non-number, naming
    /// the variable and its default (`refinement-default` ask 6, R8).
    #[test]
    fn a_zero_or_non_number_limit_refuses_the_boot_naming_the_default() {
        let var = "ORDO_REFINE_MAX_IN_VALUES";
        assert_eq!(refine_limit(var, None, 200).unwrap(), 200);
        assert_eq!(refine_limit(var, Some(" 500 "), 200).unwrap(), 500);
        for raw in ["0", "many", "-1", "", "1.5"] {
            let e = refine_limit(var, Some(raw), 200)
                .expect_err(raw)
                .to_string();
            assert!(e.starts_with(var), "{e}");
            assert!(e.contains("the default, 200"), "{e}");
        }
        let e = refine_limit(var, Some("0"), 200).unwrap_err().to_string();
        assert!(e.contains("no off switch"), "{e}");
    }
}

#[cfg(test)]
mod sync_pacing_settings {
    use super::*;

    #[test]
    fn unset_is_the_default_and_each_setting_is_parsed() {
        let none = |_: &str| None;
        let p = sync_pacing_from(&none).unwrap();
        assert_eq!(p, ordo_api::SyncPacing::default());
        assert_eq!((p.page_rows, p.page_wait), (50, Duration::from_millis(50)));
        let env = |k: &str| match k {
            "ORDO_SYNC_PAGE_ROWS" => Some(" 200 ".to_string()),
            "ORDO_SYNC_PAGE_WAIT_MS" => Some("0".to_string()),
            _ => None,
        };
        let p = sync_pacing_from(&env).unwrap();
        assert_eq!((p.page_rows, p.page_wait), (200, Duration::ZERO));
        let env = |k: &str| match k {
            "ORDO_SYNC_PAGE_ROWS" => Some("1000".to_string()),
            "ORDO_SYNC_PAGE_WAIT_MS" => Some("10000".to_string()),
            _ => None,
        };
        let p = sync_pacing_from(&env).unwrap();
        assert_eq!((p.page_rows, p.page_wait), (1000, Duration::from_secs(10)));
    }

    #[test]
    fn a_bad_page_rows_refuses_the_boot_naming_it() {
        for raw in ["0", "1001", "x", "-1", "1.5"] {
            let env = move |k: &str| (k == "ORDO_SYNC_PAGE_ROWS").then(|| raw.to_string());
            let e = sync_pacing_from(&env).expect_err(raw).to_string();
            assert!(e.contains("ORDO_SYNC_PAGE_ROWS"), "{e}");
            assert!(e.contains("from 1 to 1000"), "{e}");
            assert!(e.contains("the default, 50"), "{e}");
        }
    }

    #[test]
    fn a_bad_page_wait_refuses_the_boot_naming_it() {
        for raw in ["10001", "x", "-1", "0.5"] {
            let env = move |k: &str| (k == "ORDO_SYNC_PAGE_WAIT_MS").then(|| raw.to_string());
            let e = sync_pacing_from(&env).expect_err(raw).to_string();
            assert!(e.contains("ORDO_SYNC_PAGE_WAIT_MS"), "{e}");
            assert!(e.contains("from 0 to 10000"), "{e}");
            assert!(e.contains("the default, 50"), "{e}");
        }
    }
}

#[cfg(test)]
mod fixed_values_as_settings {
    use super::*;

    #[test]
    fn a_bad_value_refuses_the_boot_naming_the_setting_and_the_default() {
        let none = |_: &str| None;
        assert_eq!(
            bounded_setting(&none, "ORDO_AUTH_PROVIDER_HANDOFF_SECS", 60, 10, 600).unwrap(),
            60
        );
        for raw in ["0", "9", "601", "x", "-1", "1.5"] {
            let env = move |_: &str| Some(raw.to_string());
            let e = bounded_setting(&env, "ORDO_AUTH_PROVIDER_HANDOFF_SECS", 60, 10, 600)
                .expect_err(raw)
                .to_string();
            assert!(e.contains("ORDO_AUTH_PROVIDER_HANDOFF_SECS"), "{e}");
            assert!(e.contains("the default, 60"), "{e}");
        }
        let env = |_: &str| Some(" 90 ".to_string());
        assert_eq!(
            bounded_setting(&env, "ORDO_AUTH_PROVIDER_HANDOFF_SECS", 60, 10, 600).unwrap(),
            90
        );
    }
}

#[cfg(test)]
mod json_limits {
    use super::*;

    /// The boot refuses a json size or depth limit of 0 or a non-number,
    /// naming the variable and its default (28b).
    #[test]
    fn a_zero_or_non_number_json_limit_refuses_the_boot_naming_the_default() {
        let var = "ORDO_JSON_MAX_BYTES";
        assert_eq!(json_limit(var, None, 262144).unwrap(), 262144);
        assert_eq!(json_limit(var, Some("1048576"), 262144).unwrap(), 1048576);
        for raw in ["0", "big", "-1", ""] {
            let e = json_limit(var, Some(raw), 262144)
                .expect_err(raw)
                .to_string();
            assert!(e.starts_with(var), "{e}");
            assert!(e.contains("the default, 262144"), "{e}");
        }
    }
}

#[cfg(test)]
mod registration {
    use super::*;

    /// **`std.space.attachBucket` is registered only where an attachment is
    /// honoured** (18p U6).
    ///
    /// A unit test on the filter rather than only a boot test, because the
    /// boot test can drive one configuration per process and the property is
    /// about the gate: the op appears when the gate is true and not when it
    /// is false, and every other op in the same `filter` is unaffected
    /// either way.
    ///
    /// What a regression looks like: the op registered on a `single`
    /// deployment, where every upload still resolves to the one
    /// `ORDO_BLOB_STORE`. The door would accept an attachment, record a
    /// custody change, print the consent sentence saying the objects live in
    /// the tenant's bucket, and put every byte in the platform's — worse
    /// than the op being absent, because a false custody disclosure is a
    /// promise rather than a missing feature.
    /// `std.space.import` is registered exactly when a bundle has somewhere
    /// to be staged, and its gate is its own.
    ///
    /// The `PlatformOps` invariant is that a registered op has state behind
    /// it. An import registered on a deployment with no staging root would
    /// answer `internal` to an owner who had done nothing wrong, and it would
    /// answer it after the door had already told the caller the op exists.
    ///
    /// The second half of the assertion is that the gate is **separate** from
    /// the exit ops'. A deployment can legitimately serve one and not the
    /// other: letting a tenant leave is a promise, and letting anyone stage
    /// bytes on the deployment's disk is an operator's decision.
    #[test]
    fn the_import_op_is_registered_only_where_a_bundle_can_be_staged() {
        let names = |imports| -> Vec<String> {
            platform_specs(true, imports, true, true)
                .into_iter()
                .map(|s| s.name)
                .collect()
        };
        let on = names(true);
        let off = names(false);
        assert!(on.iter().any(|n| n == crate::exit_ops::IMPORT));
        assert!(!off.iter().any(|n| n == crate::exit_ops::IMPORT));
        assert_eq!(on.len(), off.len() + 1, "the gate is this op's alone");
        // The exit ops are gated on their own directory and must be present
        // either way, which is what "separate gate" means.
        for other in [crate::exit_ops::EXPORT, crate::exit_ops::DELETE] {
            assert!(off.iter().any(|n| n == other), "{other}");
        }
    }

    #[test]
    fn the_attach_bucket_op_is_registered_only_where_an_attachment_is_honoured() {
        let names = |buckets| -> Vec<String> {
            platform_specs(true, true, true, buckets)
                .into_iter()
                .map(|s| s.name)
                .collect()
        };
        let on = names(true);
        let off = names(false);
        for op in [
            crate::bucket_ops::ATTACH_BUCKET,
            crate::bucket_ops::DETACH_BUCKET,
        ] {
            assert!(on.iter().any(|n| n == op), "{op}");
            assert!(!off.iter().any(|n| n == op), "{op}");
        }
        // And nothing else moved with them: the gate is the attach and the
        // detach's alone, and the two go together.
        assert_eq!(on.len(), off.len() + 2);
        for other in [
            crate::platform_ops::CALLERS_OF,
            crate::apply_op::APPLY,
            crate::exit_ops::EXPORT,
            crate::exit_ops::IMPORT,
            crate::exit_ops::DELETE,
            crate::attest_ops::ATTEST_TO,
            crate::attest_ops::CHAIN_PROOF,
        ] {
            assert!(off.iter().any(|n| n == other), "{other}");
        }
    }

    /// Enrolment E3 to E5: every membership and admission op the platform
    /// defines is registered on a deployment, whatever else is switched off.
    /// `platform_specs` is a filter that answers `false` for a name it does
    /// not know, so an op added to `platform_ops::specs` and not named here
    /// exists in the code and is a 404 on the binary.
    /// Every op `platform_ops::specs` defines is served when every feature is
    /// on. `platform_specs` answers `false` for a name it does not know, so
    /// without this an op can be defined, wired and tested through a harness
    /// that registers `specs()` directly, and still be a 404 on the binary.
    /// That happened to `std.space.create` and `std.space.reap`.
    #[test]
    fn every_platform_op_is_served_when_every_feature_is_on() {
        let served: Vec<String> = platform_specs(true, true, true, true)
            .into_iter()
            .map(|s| s.name)
            .collect();
        for spec in crate::platform_ops::specs() {
            assert!(
                served.contains(&spec.name),
                "{} is defined and not served",
                spec.name
            );
        }
    }

    #[test]
    fn the_membership_and_admission_ops_are_registered_whatever_is_switched_off() {
        let served: Vec<String> = platform_specs(false, false, false, false)
            .into_iter()
            .map(|s| s.name)
            .collect();
        let wanted = crate::member_ops::OPS
            .iter()
            .copied()
            .chain(crate::identity_ops::OPS)
            .chain(crate::profile_ops::OPS)
            .chain(crate::people_ops::OPS)
            .chain([crate::admission::SET_ADMISSION, crate::admission::ADMISSION]);
        for op in wanted {
            assert!(served.iter().any(|n| n == op), "{op} is not registered");
        }
    }
}

#[cfg(test)]
mod provider_roots_tests {
    use super::{provider_roots, BootRefusal};

    const CA_PEM: &[u8] = include_bytes!("../tests/fixture/ca.pem");

    fn with(value: Option<&str>, file: std::io::Result<Vec<u8>>) -> Result<(), BootRefusal> {
        let file = std::sync::Mutex::new(Some(file));
        provider_roots(
            &|k| {
                (k == "ORDO_AUTH_PROVIDER_CA_FILE")
                    .then(|| value.map(str::to_string))
                    .flatten()
            },
            &|_| file.lock().unwrap().take().expect("read once"),
        )
        .map(|_| ())
    }

    fn refused_by_name(r: Result<(), BootRefusal>) -> String {
        match r {
            Err(BootRefusal::Invalid { var, detail }) => {
                assert_eq!(var, "ORDO_AUTH_PROVIDER_CA_FILE");
                detail
            }
            other => panic!("expected a refusal naming the setting, got {other:?}"),
        }
    }

    #[test]
    fn unset_trusts_the_public_program_and_a_pem_ca_is_added() {
        assert!(with(None, Ok(Vec::new())).is_ok());
        assert!(with(Some("  "), Ok(Vec::new())).is_ok());
        assert!(with(Some("/ca.pem"), Ok(CA_PEM.to_vec())).is_ok());
    }

    #[test]
    fn an_unreadable_or_empty_file_refuses_the_boot_by_name() {
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(refused_by_name(with(Some("/nope"), Err(missing))).contains("could not be read"));
        assert!(
            refused_by_name(with(Some("/empty"), Ok(b"not a cert".to_vec())))
                .contains("no PEM CERTIFICATE")
        );
    }
}

#[cfg(test)]
mod connect_refusal_tests {
    use super::*;

    #[test]
    fn an_unreachable_database_names_host_and_port() {
        let r = connect_refusal(
            "db: error connecting to server: Connection refused",
            "deployment-db",
            5432,
        );
        let m = r.to_string();
        assert!(m.contains("ORDO_PG_HOST:ORDO_PG_PORT"), "{m}");
        assert!(m.contains("deployment-db:5432"), "{m}");
        assert!(m.contains("Check that Postgres is running"), "{m}");
    }

    #[test]
    fn other_database_errors_are_left_alone() {
        let m = connect_refusal("db: relation is missing", "h", 1).to_string();
        assert!(!m.contains("ORDO_PG_HOST"), "{m}");
    }
}
