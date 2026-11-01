//! The deployment's settings, in one registry, and `std.deployment.settings`
//! (G7, `plan-console.md` 4.2, 4.6, 4.10 and 7).
//!
//! # One registry
//!
//! [`REGISTRY`] names every setting a deployment owner can give `ordo-host`:
//! its name, its group, its default and its allowed range, as
//! `docs/settings.md` states them. The values themselves are still read
//! where they are used, through [`crate::runtime::RuntimeConfig::from_env`]
//! and the modules it calls; the registry sits beside that one reader and
//! answers what a reader cannot: which settings exist, and which of them
//! this process took from where.
//!
//! The registry and `docs/settings.md` agree by a test
//! (`the_registry_and_the_settings_doc_agree`): a row added to one and not
//! the other fails it, and so does a setting a crate reads that neither
//! names (`every_setting_the_code_reads_is_registered`).
//!
//! A name holding `<NAME>` is a family, such as one outside sign-in
//! provider's `ORDO_AUTH_<NAME>_CLIENT_ID`: each variable that matches it is
//! one row in the answer.
//!
//! # What the op answers
//!
//! One row per setting: its effective value, default, range and source:
//!
//! | Source | Means |
//! |---|---|
//! | `default` | unset, so the default holds |
//! | `environment` | the variable `NAME` |
//! | `file` | the variable `NAME_FILE` names a file holding it (`ordo_env::secret`) |
//! | `stored` | a platform op stored a value, which wins over the environment after the first boot (guest sign-in, self-serve) |
//!
//! **A secret is shown only as set or unset** (`ordo_env::hides_value`, the
//! one list the console and the boot record use too). Its value, its file's
//! path is never in a row.
//!
//! The app developer's layers that platform ops store with no one
//! environment setting behind them (session policy, email policy, link
//! password limits) are rows of their own, `kind: "layer"`.
//!
//! The op is the deployment owner's and answers only in the deployment's own
//! space ([`crate::platform_ops::owner_only`]). It reads this process's
//! environment, so with several nodes it answers for the node that served it.

use std::collections::BTreeMap;

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    RefineTier, Syncability,
};
use ordo_exec::{Pool, PoolError};
use serde_json::{json, Value};

/// `std.deployment.settings`.
pub const SETTINGS: &str = "std.deployment.settings";
/// Every op this module serves.
pub const OPS: [&str; 1] = [SETTINGS];

/// The group a variable falls in when the registry does not name it.
pub const UNLISTED: &str = "Not in the registry";

/// One setting a deployment owner can give `ordo-host`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    /// The variable, or a family with `<NAME>` in it.
    pub name: &'static str,
    /// The section of `docs/settings.md` it is in.
    pub group: &'static str,
    /// The default, as `docs/settings.md` words it.
    pub default: &'static str,
    /// The values it takes, when the doc states them briefly. Empty: see
    /// `docs/settings.md`.
    pub range: &'static str,
}

const fn s(
    name: &'static str,
    group: &'static str,
    default: &'static str,
    range: &'static str,
) -> Setting {
    Setting {
        name,
        group,
        default,
        range,
    }
}

/// Every deployment setting, in the order of `docs/settings.md`. A secret
/// is listed by its own name; its `_FILE` form is the same setting.
pub static REGISTRY: &[Setting] = &[
    s(
        "ORDO_PUBLIC_CACHE_BYTES",
        "Public pages and the proxy",
        r#"268435456 (256 MiB)"#,
        r#"0 to 68719476736"#,
    ),
    s(
        "ORDO_PUBLIC_CDN_S_MAXAGE",
        "Public pages and the proxy",
        r#"60"#,
        r#"0 to 604800"#,
    ),
    s(
        "ORDO_PUBLIC_CDN_SWR",
        "Public pages and the proxy",
        r#"300"#,
        r#"0 to 604800"#,
    ),
    s(
        "ORDO_PUBLIC_MISSES_PER_MINUTE",
        "Public pages and the proxy",
        r#"600"#,
        r#"0 to 1000000"#,
    ),
    s(
        "ORDO_PUBLIC_CDN_PURGE_URL",
        "Public pages and the proxy",
        r#"unset"#,
        r#""#,
    ),
    s(
        "ORDO_PUBLIC_CDN_PURGE_SECRET",
        "Public pages and the proxy",
        r#"unset"#,
        r#""#,
    ),
    s(
        "ORDO_PUBLIC_CDN_PURGE_TIMEOUT_SECS",
        "Public pages and the proxy",
        r#"5"#,
        r#"1 to 60"#,
    ),
    s(
        "ORDO_PUBLIC_WRITES_PER_HOUR",
        "Public pages and the proxy",
        r#"10000"#,
        r#"0 to 1000000"#,
    ),
    s(
        "ORDO_PUBLIC_HUMAN_CHECK",
        "Public pages and the proxy",
        r#"on"#,
        r#"`on` or `off`"#,
    ),
    s(
        "ORDO_PUBLIC_HUMAN_PROVIDER",
        "Public pages and the proxy",
        r#"turnstile"#,
        r#"`turnstile`, `hcaptcha` or `recaptcha`"#,
    ),
    s(
        "ORDO_PUBLIC_HUMAN_SECRET",
        "Public pages and the proxy",
        r#"`ORDO_AUTH_HUMAN_CHECK_SECRET_FILE`"#,
        r#""#,
    ),
    s(
        "ORDO_PUBLIC_HUMAN_VERIFY_URL",
        "Public pages and the proxy",
        r#"`ORDO_AUTH_HUMAN_CHECK_VERIFY_URL`, then the provider's"#,
        r#""#,
    ),
    s(
        "ORDO_PUBLIC_HUMAN_VERIFY_SECS",
        "Public pages and the proxy",
        r#"`ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS`, then 5"#,
        r#"1 to 30"#,
    ),
    s(
        "ORDO_PROXY_HEADER",
        "Public pages and the proxy",
        r#"unset (proxy mode off)"#,
        r#""#,
    ),
    s(
        "ORDO_PROXY_TRUSTED",
        "Public pages and the proxy",
        r#"unset"#,
        r#""#,
    ),
    s(
        "ORDO_PROXY_SLOW_AFTER",
        "Public pages and the proxy",
        r#"120"#,
        r#"0 to 1000000"#,
    ),
    s(
        "ORDO_PROXY_SLOW_MS",
        "Public pages and the proxy",
        r#"250"#,
        r#"1 to 60000"#,
    ),
    s(
        "ORDO_PROXY_SLOW_MAX_MS",
        "Public pages and the proxy",
        r#"5000"#,
        r#"0 to 60000"#,
    ),
    s(
        "ORDO_AUTH_METHODS",
        "Sign-in and sessions",
        r#"unset: no sign-in"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_METHODS_MAX_AGE",
        "Sign-in and sessions",
        r#"60"#,
        r#"0 to 3600"#,
    ),
    s(
        "ORDO_SESSION_SECS",
        "Sign-in and sessions",
        r#"3600 s"#,
        r#""#,
    ),
    s(
        "ORDO_SESSION_MAX_SECS",
        "Sign-in and sessions",
        r#"43200 s"#,
        r#""#,
    ),
    s(
        "ORDO_SESSION_SECS_LIMIT",
        "Sign-in and sessions",
        r#"86400 s"#,
        r#""#,
    ),
    s(
        "ORDO_SESSION_MAX_SECS_LIMIT",
        "Sign-in and sessions",
        r#"2592000 s (30 days)"#,
        r#""#,
    ),
    s(
        "ORDO_REMEMBER_SECS_LIMIT",
        "Sign-in and sessions",
        r#"34560000 s (400 days)"#,
        r#""#,
    ),
    s(
        "ORDO_REMEMBER_MAX_SECS",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_REMEMBER_MAX_SECS_LIMIT",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_REMEMBER",
        "Sign-in and sessions",
        r#"`on`"#,
        r#"`on` or `off`"#,
    ),
    s(
        "ORDO_REMEMBER_SECS",
        "Sign-in and sessions",
        r#"2592000 s (30 days)"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_PROVISIONING",
        "Sign-in and sessions",
        r#"`invite-only`"#,
        r#"`invite-only` or `auto-onboard`"#,
    ),
    s(
        "ORDO_AUTH_HUMAN_CHECK",
        "Sign-in and sessions",
        r#"`off`"#,
        r#"`turnstile` or `off`"#,
    ),
    s(
        "ORDO_AUTH_HUMAN_CHECK_SECRET",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_HUMAN_CHECK_SITE_KEY",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_HUMAN_CHECK_VERIFY_URL",
        "Sign-in and sessions",
        r#"`https://challenges.cloudflare.com/turnstile/v0/siteverify`"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS",
        "Sign-in and sessions",
        r#"5"#,
        r#"1 to 30"#,
    ),
    s(
        "ORDO_AUTH_EMAIL_CODE_DIGITS",
        "Sign-in and sessions",
        r#"8"#,
        r#"6 to 10"#,
    ),
    s(
        "ORDO_AUTH_EMAIL_CODE_SECS",
        "Sign-in and sessions",
        r#"600"#,
        r#"120 to 900"#,
    ),
    s(
        "ORDO_AUTH_EMAIL_CODE_ATTEMPTS",
        "Sign-in and sessions",
        r#"5"#,
        r#"1 to 10"#,
    ),
    s(
        "ORDO_AUTH_EMAIL_PER_HOUR",
        "Sign-in and sessions",
        r#"5"#,
        r#"1 to 1000"#,
    ),
    s(
        "ORDO_AUTH_EMAIL_PER_DAY",
        "Sign-in and sessions",
        r#"15"#,
        r#"1 to 10000"#,
    ),
    s(
        "ORDO_AUTH_EMAIL_DEVICE_PER_HOUR",
        "Sign-in and sessions",
        r#"3"#,
        r#"0 to 1000"#,
    ),
    s(
        "ORDO_AUTH_EMAIL_DEVICE_DAYS",
        "Sign-in and sessions",
        r#"30"#,
        r#"1 to 400"#,
    ),
    s(
        "ORDO_EMAIL_REQUIRED",
        "Sign-in and sessions",
        r#"off"#,
        r#""#,
    ),
    s(
        "ORDO_EMAIL_CHANGE_TOUCH",
        "Sign-in and sessions",
        r#"on"#,
        r#"`on` or `off`"#,
    ),
    s(
        "ORDO_AUTH_EMAIL_DEPLOYMENT_PER_HOUR",
        "Sign-in and sessions",
        r#"1000"#,
        r#"1 to 10,000,000"#,
    ),
    s(
        "ORDO_AUTH_REGISTRATION_TTL_SECS",
        "Sign-in and sessions",
        r#"90 days"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_TEMPORARY_PASSWORDS",
        "Sign-in and sessions",
        r#"`off`"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_SECOND_FACTOR",
        "Sign-in and sessions",
        r#"`enabled`"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_SECOND_STEP_ATTEMPTS",
        "Sign-in and sessions",
        r#"5"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_SECOND_FACTOR_LOCK_AFTER",
        "Sign-in and sessions",
        r#"20"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_SECOND_STEP_SECS",
        "Sign-in and sessions",
        r#"300 s"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_SECOND_FACTOR_MAX",
        "Sign-in and sessions",
        r#"10"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_REAUTH_SECS",
        "Sign-in and sessions",
        r#"300 s"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_QUARANTINE_SECS",
        "Sign-in and sessions",
        r#"86400 s (1 day)"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_CONTEST_SECS",
        "Sign-in and sessions",
        r#"259200 s (3 days)"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_HOST_DELAY_SECS",
        "Sign-in and sessions",
        r#"0"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_CODE_DELAY_SECS",
        "Sign-in and sessions",
        r#"0"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_OPERATOR_DELAY_SECS",
        "Sign-in and sessions",
        r#"86400 s"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_SUCCESSION_DELAY_SECS",
        "Sign-in and sessions",
        r#"86400 s"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_TENANT_DELAY_SECS",
        "Sign-in and sessions",
        r#"0"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_OPERATOR_APPROVALS",
        "Sign-in and sessions",
        r#"1"#,
        r#"1 to 10"#,
    ),
    s(
        "ORDO_RECOVERY_TENANT_APPROVALS",
        "Sign-in and sessions",
        r#"1"#,
        r#"1 to 10"#,
    ),
    s(
        "ORDO_RECOVERY_PATHS",
        "Sign-in and sessions",
        r#"all four"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_HOLD_DEFAULT",
        "Sign-in and sessions",
        r#"off"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_MANAGERS",
        "Sign-in and sessions",
        r#"off"#,
        r#""#,
    ),
    s(
        "ORDO_RECOVERY_TICKET_SECS",
        "Sign-in and sessions",
        r#"259200 s (3 days)"#,
        r#"3600 to 2592000"#,
    ),
    s(
        "ORDO_RECOVERY_CODES_PER_SET",
        "Sign-in and sessions",
        r#"10"#,
        r#"1 to 50"#,
    ),
    s(
        "ORDO_SUCCESSION_ENDS_GRANTS",
        "Sign-in and sessions",
        r#"on"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_RETURN_URLS",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_HANDOFF_SECS",
        "Sign-in and sessions",
        r#"60"#,
        r#"1 to 300"#,
    ),
    s(
        "ORDO_AUTH_LINKING",
        "Sign-in and sessions",
        r#"on"#,
        r#"`on` or `off`"#,
    ),
    s(
        "ORDO_AUTH_LINKING_SECS",
        "Sign-in and sessions",
        r#"300"#,
        r#"60 to 900"#,
    ),
    s(
        "ORDO_AUTH_GUEST",
        "Sign-in and sessions",
        r#"off"#,
        r#"`on` or `off`"#,
    ),
    s(
        "ORDO_AUTH_GUEST_TTL_SECS",
        "Sign-in and sessions",
        r#"the registration lifetime (90 days)"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_CONSOLE_SESSION_SECS",
        "Sign-in and sessions",
        r#"900"#,
        r#"60 to 3600"#,
    ),
    s(
        "ORDO_IDENTITY_URL",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_IDENTITY_ORIGIN",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_IDENTITY_DEPLOYMENT",
        "Sign-in and sessions",
        r#"main"#,
        r#"lowercase letters, digits and dashes, at most 63"#,
    ),
    s(
        "ORDO_IDENTITY_PLAIN_HTTP",
        "Sign-in and sessions",
        r#"off"#,
        r#"on or off"#,
    ),
    s(
        "ORDO_IDENTITY_TOKEN_SECS",
        "Sign-in and sessions",
        r#"600"#,
        r#"60 to 3600"#,
    ),
    s(
        "ORDO_IDENTITY_BASE_PATH",
        "Sign-in and sessions",
        r#"unset (the root)"#,
        r#"/ and lower-case segments"#,
    ),
    s(
        "ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER",
        "Sign-in and sessions",
        r#"false"#,
        r#"true, false"#,
    ),
    s(
        "ORDO_RUNTIME_REVISION",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_RUNTIME_APPLIED_BY",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_SUPERVISOR_IDENTITY_USER",
        "Sign-in and sessions",
        r#"unset"#,
        r#"uid:gid, not 0"#,
    ),
    s(
        "ORDO_SUPERVISOR_CONSOLE_USER",
        "Sign-in and sessions",
        r#"unset"#,
        r#"uid:gid, not 0"#,
    ),
    s(
        "ORDO_SUPERVISOR_DEPLOYMENT_USER",
        "Sign-in and sessions",
        r#"unset"#,
        r#"uid:gid, not 0"#,
    ),
    s(
        "ORDO_SUPERVISOR_IDENTITY_BIN",
        "Sign-in and sessions",
        r#"/usr/local/bin/ordo-identity-service"#,
        r#"a path"#,
    ),
    s(
        "ORDO_SUPERVISOR_CONSOLE_BIN",
        "Sign-in and sessions",
        r#"/usr/local/bin/ordo-console"#,
        r#"a path"#,
    ),
    s(
        "ORDO_SUPERVISOR_HOST_BIN",
        "Sign-in and sessions",
        r#"/usr/local/bin/ordo-host"#,
        r#"a path"#,
    ),
    s(
        "ORDO_SUPERVISOR_STATE_DIR",
        "Sign-in and sessions",
        r#"/var/lib/ordo-supervisor"#,
        r#"a folder"#,
    ),
    s(
        "ORDO_SUPERVISOR_KEY_FILE",
        "Sign-in and sessions",
        r#"<state dir>/supervisor.key"#,
        r#"a path"#,
    ),
    s(
        "ORDO_SUPERVISOR_RUN_DIR",
        "Sign-in and sessions",
        r#"/run/ordo"#,
        r#"a folder"#,
    ),
    s(
        "ORDO_SUPERVISOR_SOCKET",
        "Sign-in and sessions",
        r#"<run dir>/supervisor.sock"#,
        r#"a path"#,
    ),
    s(
        "ORDO_SUPERVISOR_LISTEN",
        "Sign-in and sessions",
        r#"127.0.0.1:8079"#,
        r#"address:port"#,
    ),
    s(
        "ORDO_SUPERVISOR_GRACE_SECS",
        "Sign-in and sessions",
        r#"30"#,
        r#"1 or more"#,
    ),
    s(
        "ORDO_SUPERVISOR_READY_SECS",
        "Sign-in and sessions",
        r#"120"#,
        r#"5 or more"#,
    ),
    s(
        "ORDO_SUPERVISOR_RELAY",
        "Sign-in and sessions",
        r#"auto"#,
        r#"auto, always"#,
    ),
    s(
        "ORDO_SUPERVISOR_IDENTITY_ADDRESS",
        "Sign-in and sessions",
        r#"console-path"#,
        r#"console-path, own-port"#,
    ),
    s(
        "ORDO_IMAGE_DIGEST",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_WILDCARD_ORIGINS",
        "Sign-in and sessions",
        r#"off"#,
        r#"`on` or `off`"#,
    ),
    s("ORDO_PUBLIC_URL", "Sign-in and sessions", r#"none"#, r#""#),
    s(
        "ORDO_AUTH_<NAME>_PRESET",
        "Sign-in and sessions",
        r#"none"#,
        r#"`discord` or `github` (OAuth2); `google` or `microsoft` (OIDC)"#,
    ),
    s(
        "ORDO_AUTH_<NAME>_KIND",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_CLIENT_ID",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_CLIENT_SECRET",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_AUTHORIZE_URL",
        "Sign-in and sessions",
        r#"the preset's"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_TOKEN_URL",
        "Sign-in and sessions",
        r#"the preset's"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_USERINFO_URL",
        "Sign-in and sessions",
        r#"the preset's"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_SCOPE",
        "Sign-in and sessions",
        r#"the preset's"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_SUBJECT_CLAIM",
        "Sign-in and sessions",
        r#"the preset's"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_EMAIL_CLAIM",
        "Sign-in and sessions",
        r#"the preset's"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_EMAIL_VERIFIED_CLAIM",
        "Sign-in and sessions",
        r#"the preset's"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_DISPLAY_CLAIMS",
        "Sign-in and sessions",
        r#"the preset's"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_AVATAR_CLAIM",
        "Sign-in and sessions",
        r#"the preset's"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_ISSUER",
        "Sign-in and sessions",
        r#"Google's, with the preset"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_TENANT",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_USE_EMAIL",
        "Sign-in and sessions",
        r#"on; off for Microsoft"#,
        r#"`on` or `off`"#,
    ),
    s(
        "ORDO_AUTH_<NAME>_JWKS_URL",
        "Sign-in and sessions",
        r#"none: discovery"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_ISS_PARAMETER",
        "Sign-in and sessions",
        r#"unset"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_CLOCK_SKEW_SECS",
        "Sign-in and sessions",
        r#"`ORDO_AUTH_OIDC_CLOCK_SKEW_SECS` (60)"#,
        r#"0 to 120"#,
    ),
    s(
        "ORDO_AUTH_<NAME>_ALGS",
        "Sign-in and sessions",
        r#"`ORDO_AUTH_OIDC_ALGS` (all four)"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_OIDC_CACHE_SECS",
        "Sign-in and sessions",
        r#"3600"#,
        r#"300 to 86400"#,
    ),
    s(
        "ORDO_AUTH_OIDC_CLOCK_SKEW_SECS",
        "Sign-in and sessions",
        r#"60"#,
        r#"0 to 120"#,
    ),
    s(
        "ORDO_AUTH_OIDC_ALGS",
        "Sign-in and sessions",
        r#"all four"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_OIDC_STALE_GRACE_SECS",
        "Sign-in and sessions",
        r#"86400 (24 h)"#,
        r#"0 to 604800"#,
    ),
    s(
        "ORDO_AUTH_TENANT_LOCK_WAIT_MS",
        "Sign-in and sessions",
        r#"10000"#,
        r#"100 to 120000"#,
    ),
    s(
        "ORDO_AUTH_<NAME>_PRESENCE",
        "Sign-in and sessions",
        r#"off"#,
        r#"`off` or `on`"#,
    ),
    s(
        "ORDO_AUTH_TENANT_PROVIDERS",
        "Sign-in and sessions",
        r#"off"#,
        r#"`off` or `on`"#,
    ),
    s(
        "ORDO_AUTH_<NAME>_PRESENCE_AMR",
        "Sign-in and sessions",
        r#"`mfa`"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_<NAME>_PRESENCE_ACR",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_UPSTREAM_PRESENCE_SECS",
        "Sign-in and sessions",
        r#"300"#,
        r#"30 to 300"#,
    ),
    s(
        "ORDO_AUTH_<NAME>_MUTABLE_SUBJECT",
        "Sign-in and sessions",
        r#"refuses"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_PROVIDER_HANDOFF_SECS",
        "Sign-in and sessions",
        r#"60"#,
        r#"10 to 600"#,
    ),
    s(
        "ORDO_AUTH_PASSWORD_PENDING_MAX",
        "Sign-in and sessions",
        r#"10000"#,
        r#"10 to 1000000"#,
    ),
    s(
        "ORDO_PASSWORD_SLOW_AFTER",
        "Sign-in and sessions",
        r#"5 (owner, 2026-10-25)"#,
        r#""#,
    ),
    s(
        "ORDO_PASSWORD_SLOW_AFTER_LIMIT",
        "Sign-in and sessions",
        r#"100"#,
        r#"1 to 1000"#,
    ),
    s(
        "ORDO_PASSWORD_SLOW_CAP_SECS",
        "Sign-in and sessions",
        r#"60"#,
        r#""#,
    ),
    s(
        "ORDO_PASSWORD_SLOW_CAP_SECS_LIMIT",
        "Sign-in and sessions",
        r#"1"#,
        r#"1 to 86400"#,
    ),
    s(
        "ORDO_AUTH_PROVIDER_REQUEST_SECS",
        "Sign-in and sessions",
        r#"10"#,
        r#"1 to 60"#,
    ),
    s(
        "ORDO_AUTH_PROVIDER_CA_FILE",
        "Sign-in and sessions",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_CLAIM_CODE",
        "Enrolment and admission",
        r#"removed"#,
        r#""#,
    ),
    s(
        "ORDO_PROVISION_SELF_SERVE",
        "Enrolment and admission",
        r#"`off`"#,
        r#""#,
    ),
    s(
        "ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR",
        "Enrolment and admission",
        r#"off"#,
        r#"1 to 1000000"#,
    ),
    s(
        "ORDO_PROVISION_SIGNUPS_PER_HOUR",
        "Enrolment and admission",
        r#"60"#,
        r#""#,
    ),
    s(
        "ORDO_DEPLOYMENT_OWNERS",
        "Enrolment and admission",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_APEX_DOMAIN",
        "Passkeys and presence",
        r#"none, required with passkey"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_ORIGINS",
        "Passkeys and presence",
        r#"none, required with passkey"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_RELATED_ORIGINS",
        "Passkeys and presence",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_RP_NAME",
        "Passkeys and presence",
        r#"the relying party id"#,
        r#""#,
    ),
    s(
        "ORDO_AUTH_USER_VERIFICATION",
        "Passkeys and presence",
        r#"`required`"#,
        r#""#,
    ),
    s(
        "ORDO_PRESENCE_ON_READS",
        "Passkeys and presence",
        r#"`off`"#,
        r#"`on` or `off`"#,
    ),
    s(
        "ORDO_STEP_UP_EXEMPT_OPS",
        "Passkeys and presence",
        r#"none: every change needs it"#,
        r#""#,
    ),
    s(
        "ORDO_DELEGABLE_OPS",
        "Passkeys and presence",
        r#"none: every delete and every platform write is closed to delegated tokens"#,
        r#""#,
    ),
    s(
        "ORDO_ACCESS_TEMPLATE",
        "Passkeys and presence",
        r#"none: every role calls authored reads, editors and up authored writes, managers and owners the `std.*` ops"#,
        r#""#,
    ),
    s(
        "ORDO_DELEGATION_AGENTS",
        "Passkeys and presence",
        r#"none: no agent may ask, and a request is refused naming this setting"#,
        r#""#,
    ),
    s(
        "ORDO_DELEGATION_MAX_HOURS",
        "Passkeys and presence",
        r#"8"#,
        r#"1 to 720"#,
    ),
    s(
        "ORDO_DELEGATION_REQUEST_SECS",
        "Passkeys and presence",
        r#"600"#,
        r#"60 to 3600"#,
    ),
    s(
        "ORDO_DELEGATION_MAX_OPS",
        "Passkeys and presence",
        r#"10"#,
        r#"1 to 100"#,
    ),
    s(
        "ORDO_DELEGATION_OPEN_PER_AGENT",
        "Passkeys and presence",
        r#"1000"#,
        r#"1 to 1000000"#,
    ),
    s(
        "ORDO_DELEGATION_OPEN_PER_PERSON",
        "Passkeys and presence",
        r#"10"#,
        r#"1 to 1000"#,
    ),
    s(
        "ORDO_KEY_PROVIDER",
        "Custody and keys",
        r#"none, required"#,
        r#""#,
    ),
    s("ORDO_MASTER_KEY", "Custody and keys", r#"none"#, r#""#),
    s(
        "ORDO_MASTER_KEY_PASSPHRASE",
        "Custody and keys",
        r#"none, required for `sealed-master`"#,
        r#""#,
    ),
    s(
        "ORDO_DEV_UNSAFE_TEST_KEYS",
        "Custody and keys",
        r#"off"#,
        r#""#,
    ),
    s(
        "ORDO_TOKEN_KEY",
        "Custody and keys",
        r#"none, required"#,
        r#""#,
    ),
    s(
        "ORDO_PAGE_KEY",
        "Custody and keys",
        r#"none, required"#,
        r#""#,
    ),
    s("ORDO_CHECKPOINT_KEY", "Custody and keys", r#"none"#, r#""#),
    s(
        "ORDO_CHECKPOINT_KEY_ID",
        "Custody and keys",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_CHECKPOINT_PUBKEY",
        "Custody and keys",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_SECURITY_CHECKPOINT_KEY",
        "Custody and keys",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_SECURITY_CHECKPOINT_KEY_ID",
        "Custody and keys",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_SECURITY_CHECKPOINT_PUBKEY",
        "Custody and keys",
        r#"none"#,
        r#""#,
    ),
    s("ORDO_RESEAL_CHUNK", "Custody and keys", r#"256"#, r#""#),
    s(
        "ORDO_RESEAL_PAUSE_MS",
        "Custody and keys",
        r#"250 ms"#,
        r#""#,
    ),
    s("ORDO_DECRYPT_SEEN_SECS", "Custody and keys", r#"60"#, r#""#),
    s("ORDO_SHARE_LINKS", "Sharing", r#"`on`"#, r#""#),
    s(
        "ORDO_SHARE_LINK_DEFAULT_SECS",
        "Sharing",
        r#"7 days"#,
        r#""#,
    ),
    s(
        "ORDO_SHARE_LINK_TENANT_LIFETIME",
        "Sharing",
        r#"`off`"#,
        r#""#,
    ),
    s(
        "ORDO_SHARE_LINK_MISS_LIMIT",
        "Sharing",
        r#"off"#,
        r#"1 to 1000000"#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_TRIES",
        "Sharing",
        r#"off (owner, 2026-10-22)"#,
        r#""#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_TRIES_LIMIT",
        "Sharing",
        r#"100"#,
        r#"1 to 1000"#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_LOCK_SECS",
        "Sharing",
        r#"900 (15 minutes)"#,
        r#""#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_LOCK_SECS_LIMIT",
        "Sharing",
        r#"60"#,
        r#"1 to 2592000"#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH",
        "Sharing",
        r#"8 (owner, 2026-10-25)"#,
        r#""#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH_LIMIT",
        "Sharing",
        r#"8"#,
        r#"1 to 256"#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_SLOW_AFTER",
        "Sharing",
        r#"5 (owner, 2026-10-25)"#,
        r#""#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_SLOW_AFTER_LIMIT",
        "Sharing",
        r#"100"#,
        r#"1 to 1000"#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_SLOW_CAP_SECS",
        "Sharing",
        r#"60"#,
        r#""#,
    ),
    s(
        "ORDO_SHARE_LINK_PASSWORD_SLOW_CAP_SECS_LIMIT",
        "Sharing",
        r#"1"#,
        r#"1 to 86400"#,
    ),
    s("ORDO_SHARE_MAX_SECS", "Sharing", r#"none: unset"#, r#""#),
    s(
        "ORDO_MEMBER_LEFT_NAME",
        "Tenants and provisioning",
        r#"unset, the name is kept"#,
        r#""#,
    ),
    s(
        "ORDO_ACCOUNT_REMOVAL",
        "Tenants and provisioning",
        r#"`never`"#,
        r#""#,
    ),
    s(
        "ORDO_SPACE",
        "Tenants and provisioning",
        r#"none, required"#,
        r#""#,
    ),
    s(
        "ORDO_DEPLOYMENT_SPACE",
        "Tenants and provisioning",
        r#"the fixture tenant"#,
        r#""#,
    ),
    s(
        "ORDO_PG_ROUTING",
        "Tenants and provisioning",
        r#"removed"#,
        r#""#,
    ),
    s(
        "ORDO_PG_TENANT_HOST",
        "Tenants and provisioning",
        r#"the deployment's own connection"#,
        r#""#,
    ),
    s(
        "ORDO_PG_TENANT_PORT",
        "Tenants and provisioning",
        r#"the deployment's own connection"#,
        r#""#,
    ),
    s(
        "ORDO_PG_TENANT_USER",
        "Tenants and provisioning",
        r#"the deployment's own connection"#,
        r#""#,
    ),
    s(
        "ORDO_PG_TENANT_CREDENTIAL",
        "Tenants and provisioning",
        r#"the deployment's own connection"#,
        r#""#,
    ),
    s(
        "ORDO_TENANT_ATTACH_HOSTS",
        "Tenants and provisioning",
        r#"none: attach refused"#,
        r#""#,
    ),
    s(
        "ORDO_PG_ATTACH_HOSTS",
        "Tenants and provisioning",
        r#"none: attach refused"#,
        r#""#,
    ),
    s(
        "ORDO_TENANT_CREDENTIALS",
        "Tenants and provisioning",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_TENANT_CREDENTIALS_DIR",
        "Tenants and provisioning",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_BOOTSTRAP_TIMEOUT_SECS",
        "Tenants and provisioning",
        r#"900 s"#,
        r#""#,
    ),
    s(
        "ORDO_PROVISION_SWEEP_SECS",
        "Tenants and provisioning",
        r#"30"#,
        r#"1 to 3600"#,
    ),
    s(
        "ORDO_PROVISION_LEASE_SECS",
        "Tenants and provisioning",
        r#"120"#,
        r#"10 to 3600"#,
    ),
    s(
        "ORDO_PROVISION_RETRY_BASE_SECS",
        "Tenants and provisioning",
        r#"15"#,
        r#"1 to 3600"#,
    ),
    s(
        "ORDO_PROVISION_RETRY_CAP_SECS",
        "Tenants and provisioning",
        r#"900"#,
        r#"1 to 86400"#,
    ),
    s(
        "ORDO_PROVISION_STUCK_SECS",
        "Tenants and provisioning",
        r#"300"#,
        r#"1 to 86400"#,
    ),
    s(
        "ORDO_PROVISION_NOT_READY_RETRY_SECS",
        "Tenants and provisioning",
        r#"2"#,
        r#"1 to 300"#,
    ),
    s(
        "ORDO_TENANT_PAUSE_ROLES",
        "Tenants and provisioning",
        r#"none: only the deployment owner"#,
        r#""#,
    ),
    s(
        "ORDO_TENANT_RESUME_ROLES",
        "Tenants and provisioning",
        r#"the pause roles"#,
        r#""#,
    ),
    s(
        "ORDO_TENANT_PAUSE_ANSWERS",
        "Tenants and provisioning",
        r#"423 and the built-in message for every reason"#,
        r#""#,
    ),
    s(
        "ORDO_PG_HOST",
        "Databases and pools",
        r#"none, required"#,
        r#""#,
    ),
    s("ORDO_PG_PORT", "Databases and pools", r#"5432"#, r#""#),
    s(
        "ORDO_PG_USER",
        "Databases and pools",
        r#"none, required"#,
        r#""#,
    ),
    s(
        "ORDO_PG_DB",
        "Databases and pools",
        r#"none, required"#,
        r#""#,
    ),
    s("ORDO_PG_PASSWORD", "Databases and pools", r#"none"#, r#""#),
    s(
        "ORDO_PG_POOL_SIZE",
        "Databases and pools",
        r#"cores, clamped to 2..8"#,
        r#""#,
    ),
    s("ORDO_PG_IDLE_SECS", "Databases and pools", r#"30 s"#, r#""#),
    s(
        "ORDO_PG_TENANT_DIAL_CONCURRENCY",
        "Databases and pools",
        r#"8"#,
        r#"1 to 64"#,
    ),
    s(
        "ORDO_PG_TENANT_DIAL_WAIT_MS",
        "Databases and pools",
        r#"60000"#,
        r#"1 to 3600000"#,
    ),
    s(
        "ORDO_PG_BUSY_SNAPSHOT",
        "Databases and pools",
        r#"on"#,
        r#"on or off"#,
    ),
    s(
        "ORDO_PG_STORE_POOL_SIZE",
        "Databases and pools",
        r#"4"#,
        r#"1 to 32"#,
    ),
    s(
        "ORDO_PG_STORE_ACQUIRE_TIMEOUT_MS",
        "Databases and pools",
        r#"10000 ms"#,
        r#"1 to 60000"#,
    ),
    s(
        "ORDO_PG_ACQUIRE_TIMEOUT_MS",
        "Databases and pools",
        r#"2000 ms"#,
        r#""#,
    ),
    s(
        "ORDO_PG_STATEMENT_TIMEOUT_MS",
        "Databases and pools",
        r#"30000 ms"#,
        r#""#,
    ),
    s(
        "ORDO_REGEX_MAX_LEN",
        "Databases and pools",
        r#"256"#,
        r#"1 to 10000"#,
    ),
    s(
        "ORDO_PG_SPACE_RESERVE",
        "Databases and pools",
        r#"0"#,
        r#""#,
    ),
    s(
        "ORDO_WRITE_RETRIES",
        "Databases and pools",
        r#"5"#,
        r#"0 to 20"#,
    ),
    s(
        "ORDO_WRITE_RETRY_MAX_MS",
        "Databases and pools",
        r#"250 ms"#,
        r#"1 to 10000"#,
    ),
    s(
        "ORDO_APPLY_LOCK_TIMEOUT_MS",
        "Databases and pools",
        r#"2000 ms"#,
        r#""#,
    ),
    s(
        "ORDO_APPLY_LOCK_RETRIES",
        "Databases and pools",
        r#"5"#,
        r#""#,
    ),
    s(
        "ORDO_APPLY_LOCK_BACKOFF_MS",
        "Databases and pools",
        r#"500 ms"#,
        r#""#,
    ),
    s(
        "ORDO_APPLY_SAFE_CHANGES",
        "Databases and pools",
        r#"on"#,
        r#""#,
    ),
    s("ORDO_BLOB_STORE", "Storage and blobs", r#"none"#, r#""#),
    s(
        "ORDO_BLOB_S3_ENDPOINT",
        "Storage and blobs",
        r#"none, required with `s3`"#,
        r#""#,
    ),
    s(
        "ORDO_BLOB_S3_REGION",
        "Storage and blobs",
        r#"none, required with `s3`"#,
        r#""#,
    ),
    s(
        "ORDO_BLOB_S3_BUCKET",
        "Storage and blobs",
        r#"none, required with `s3`"#,
        r#""#,
    ),
    s(
        "ORDO_BLOB_S3_ACCESS_KEY_ID",
        "Storage and blobs",
        r#"none, required with `s3`"#,
        r#""#,
    ),
    s(
        "ORDO_BLOB_S3_SECRET_ACCESS_KEY",
        "Storage and blobs",
        r#"none, required with `s3`"#,
        r#""#,
    ),
    s(
        "ORDO_BLOB_S3_RETRIES",
        "Storage and blobs",
        r#"2"#,
        r#"0 to 10"#,
    ),
    s(
        "ORDO_BLOB_S3_RETRY_BASE_MS",
        "Storage and blobs",
        r#"200"#,
        r#"1 to 10000"#,
    ),
    s(
        "ORDO_BLOB_ROUTING",
        "Storage and blobs",
        r#"`single`"#,
        r#""#,
    ),
    s("ORDO_BLOB_GC_SECS", "Storage and blobs", r#"3600 s"#, r#""#),
    s(
        "ORDO_DEV_UNSAFE_BLOB_STORE",
        "Storage and blobs",
        r#"off"#,
        r#""#,
    ),
    s(
        "ORDO_DEV_UNSAFE_BUCKET_ENDPOINT",
        "Storage and blobs",
        r#"off"#,
        r#""#,
    ),
    s(
        "ORDO_EXIT_BUNDLE_DIR",
        "Storage and blobs",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_FN_CACHE_DIR",
        "Storage and blobs",
        r#"$XDG_CACHE_HOME/ordo/fn, else $HOME/.cache/ordo/fn"#,
        r#"an absolute path, or off"#,
    ),
    s(
        "ORDO_FN_BLOB_BYTES",
        "Storage and blobs",
        r#"20971520 (20 MiB)"#,
        r#"0 to 1073741824"#,
    ),
    s(
        "ORDO_FN_FETCH_BODY_BYTES",
        "Storage and blobs",
        r#"33554432 (32 MiB)"#,
        r#"1024 to 268435456"#,
    ),
    s(
        "ORDO_IMPORT_BUNDLE_DIR",
        "Storage and blobs",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_OWN_DOWNLOAD_PER_HOUR",
        "Storage and blobs",
        r#"6"#,
        r#""#,
    ),
    s(
        "ORDO_OWN_DOWNLOAD_MAX_BYTES",
        "Storage and blobs",
        r#"1073741824 (1 GiB)"#,
        r#""#,
    ),
    s("ORDO_USAGE_LEDGER", "Metering and quotas", r#"off"#, r#""#),
    s(
        "ORDO_USAGE_FLUSH_SECS",
        "Metering and quotas",
        r#"60 s"#,
        r#""#,
    ),
    s(
        "ORDO_USAGE_ROLLUP_SECS",
        "Metering and quotas",
        r#"3600 s"#,
        r#""#,
    ),
    s(
        "ORDO_REPLICA_ID",
        "Metering and quotas",
        r#"8 random hex characters"#,
        r#""#,
    ),
    s(
        "ORDO_METRICS_PER_SPACE",
        "Metering and quotas",
        r#"off"#,
        r#""#,
    ),
    s(
        "ORDO_OBSERVED_COST_MAX_OPS",
        "Metering and quotas",
        r#"4096"#,
        r#""#,
    ),
    s(
        "ORDO_OBSERVED_COST_MAX_SPACE_ENTRIES",
        "Metering and quotas",
        r#"10000"#,
        r#""#,
    ),
    s(
        "ORDO_DENY_ALERT_THRESHOLD",
        "The security stream and audit",
        r#"50"#,
        r#""#,
    ),
    s(
        "ORDO_DENY_ALERT_WINDOW_SECS",
        "The security stream and audit",
        r#"300 s"#,
        r#""#,
    ),
    s(
        "ORDO_DEPLOYMENT_SECLOG_RETENTION_DAYS",
        "The security stream and audit",
        r#"90 days"#,
        r#""#,
    ),
    s(
        "ORDO_WORKER_PRUNE",
        "The security stream and audit",
        r#"off"#,
        r#""#,
    ),
    s(
        "ORDO_EGRESS_DENY_CIDRS",
        "The security stream and audit",
        r#"none"#,
        r#""#,
    ),
    s(
        "ORDO_HOST_ADDR",
        "CORS and the door",
        r#"`127.0.0.1:8080`"#,
        r#""#,
    ),
    s(
        "ORDO_CORS_ORIGINS",
        "CORS and the door",
        r#"none: same origin only"#,
        r#""#,
    ),
    s(
        "ORDO_CORS_TENANT_ORIGINS_MAX",
        "CORS and the door",
        r#"0"#,
        r#"0 to 1000"#,
    ),
    s(
        "ORDO_SHUTDOWN_GRACE_SECS",
        "CORS and the door",
        r#"30 s"#,
        r#""#,
    ),
    s(
        "ORDO_LIVE_IDLE_CLOSE_SECS",
        "CORS and the door",
        r#"60 s"#,
        r#""#,
    ),
    s(
        "ORDO_SYNC_PAGE_ROWS",
        "CORS and the door",
        r#"50"#,
        r#"1 to 1,000"#,
    ),
    s(
        "ORDO_SYNC_PAGE_WAIT_MS",
        "CORS and the door",
        r#"50 ms"#,
        r#"0 to 10,000"#,
    ),
    s("ORDO_LIVE_MAX_READS", "CORS and the door", r#"256"#, r#""#),
    s(
        "ORDO_LIVE_RECHECK_SECS",
        "CORS and the door",
        r#"30 s"#,
        r#"1 to 3,600"#,
    ),
    s(
        "ORDO_REFINE_MAX_CONDITIONS",
        "CORS and the door",
        r#"16"#,
        r#""#,
    ),
    s(
        "ORDO_REFINE_MAX_IN_VALUES",
        "CORS and the door",
        r#"200"#,
        r#""#,
    ),
    s(
        "ORDO_JSON_MAX_BYTES",
        "CORS and the door",
        r#"262144 (256 KiB)"#,
        r#""#,
    ),
    s("ORDO_JSON_MAX_DEPTH", "CORS and the door", r#"64"#, r#""#),
    s(
        "ORDO_FN_BODY_BYTES",
        "CORS and the door",
        r#"1048576 (1 MiB)"#,
        r#"1024 to 67108864"#,
    ),
    s(
        "ORDO_TENANT_CATCHUP_SECS",
        "CORS and the door",
        r#"15 s"#,
        r#""#,
    ),
    s(
        "ORDO_ROUTINE_EACH_MAX_ROWS",
        "CORS and the door",
        r#"500"#,
        r#""#,
    ),
    s("ORDO_WORKERS", "Workers and periods", r#"all"#, r#""#),
    s(
        "ORDO_WORKER_<NAME>_TICK_MS",
        "Workers and periods",
        r#"each worker's own"#,
        r#""#,
    ),
    s(
        "ORDO_ACCESS_REFRESH_SECS",
        "Workers and periods",
        r#"30 s"#,
        r#""#,
    ),
    s(
        "ORDO_DOOR_STALE_GRACE_SECS",
        "Workers and periods",
        r#"60 s"#,
        r#"0 to 86400"#,
    ),
    s(
        "ORDO_SESSION_REFRESH_SECS",
        "Workers and periods",
        r#"15 s"#,
        r#""#,
    ),
    s(
        "ORDO_TRIGGERS_IDLE_MAX_SECS",
        "Workers and periods",
        r#"10 s"#,
        r#""#,
    ),
    s("ORDO_NODE_BEAT_SECS", "Workers and periods", r#"5"#, r#""#),
    s(
        "ORDO_FN_RUN_RETENTION_DAYS",
        "Workers and periods",
        r#"14"#,
        r#"1 to 3650"#,
    ),
    s(
        "ORDO_MAIL_KIND",
        "Mail",
        r#"none (off)"#,
        r#"`smtp`, `resend` or `sendgrid`"#,
    ),
    s("ORDO_MAIL_FROM", "Mail", r#"none"#, r#""#),
    s("ORDO_MAIL_HOST", "Mail", r#"none"#, r#""#),
    s("ORDO_MAIL_PORT", "Mail", r#"465"#, r#"1 to 65535"#),
    s(
        "ORDO_MAIL_TLS",
        "Mail",
        r#"implicit"#,
        r#"`implicit`, `starttls` or `none`"#,
    ),
    s("ORDO_MAIL_CREDENTIALS", "Mail", r#"none"#, r#""#),
    s("ORDO_MAIL_KEY", "Mail", r#"none"#, r#""#),
    s("ORDO_MAIL_MAX_ATTEMPTS", "Mail", r#"5"#, r#"1 to 20"#),
    s("ORDO_MAIL_SEND_SECS", "Mail", r#"2"#, r#"1 to 60"#),
    s("ORDO_MAIL_RETRY_BASE_SECS", "Mail", r#"15"#, r#"1 to 3600"#),
    s(
        "ORDO_MAIL_RETRY_MAX_SECS",
        "Mail",
        r#"900"#,
        r#"1 to 86400"#,
    ),
    s("ORDO_MAIL_BATCH", "Mail", r#"16"#, r#"1 to 500"#),
    s("ORDO_MAIL_LEASE_SECS", "Mail", r#"120"#, r#"30 to 3600"#),
    s(
        "ORDO_MAIL_SUBJECT_MAX_BYTES",
        "Mail",
        r#"200"#,
        r#"20 to 998"#,
    ),
    s(
        "ORDO_MAIL_TEXT_MAX_BYTES",
        "Mail",
        r#"16384 (16 KiB)"#,
        r#"1024 to 1048576"#,
    ),
    s(
        "ORDO_EGRESS_PRIVATE_HOSTS",
        "Mail",
        r#"none (empty)"#,
        r#""#,
    ),
];

/// A setting a platform op stores, which wins over its variable once stored.
#[derive(Debug, Clone, Copy)]
pub struct StoredBy {
    /// The variable that seeds it on a boot that finds nothing stored.
    pub setting: &'static str,
    /// Its key in `$deploymentsetting`.
    pub key: &'static str,
    /// The op that changes it.
    pub op: &'static str,
}

/// The settings a platform op stores over their variable.
pub const STORED: [StoredBy; 3] = [
    StoredBy {
        setting: crate::auth::guest::SETTING,
        key: crate::auth::guest::KEY,
        op: crate::auth::guest::SET_GUEST,
    },
    StoredBy {
        setting: crate::self_serve::MODE_ENV,
        key: crate::self_serve::MODE_KEY,
        op: crate::self_serve::SET_SELF_SERVE,
    },
    StoredBy {
        setting: crate::self_serve::PER_HOUR_ENV,
        key: crate::self_serve::PER_HOUR_KEY,
        op: crate::self_serve::SET_SELF_SERVE,
    },
];

/// A layer the app developer stores through a platform op, with no one
/// variable behind it.
#[derive(Debug, Clone, Copy)]
pub struct Layer {
    /// Its key in `$deploymentsetting`.
    pub key: &'static str,
    /// The op that changes it.
    pub op: &'static str,
    /// What it holds, in a few words.
    pub what: &'static str,
}

/// The app developer's stored layers.
pub const LAYERS: [Layer; 4] = [
    Layer {
        key: crate::session_policy::APP_KEY,
        op: crate::session_policy::SET_APP,
        what: "How long sessions last: the app developer's defaults and ranges",
    },
    Layer {
        key: crate::contact_email::APP_KEY,
        op: crate::contact_email::SET_POLICY,
        what: "Whether a person must have a verified email address",
    },
    Layer {
        key: crate::share_links::APP_LIMITS_KEY,
        op: crate::share_links::SET_APP_PASSWORD_LIMITS,
        what: "Link password limits: the app developer's layer",
    },
    Layer {
        key: crate::guess_limits::APP_KEY,
        op: crate::guess_limits::SET_APP,
        what: "The password slowdown: the app developer's layer",
    },
];

/// The group stored layers are shown under.
pub const LAYER_GROUP: &str = "Set live by the app developer";

/// Whether `name` is in the family `pattern` (a name with `<NAME>`). The
/// part `<NAME>` stands for is letters and digits, no underscore, so a
/// family never swallows a setting of its own.
fn in_family(pattern: &str, name: &str) -> bool {
    let Some((pre, post)) = pattern.split_once("<NAME>") else {
        return false;
    };
    name.len() > pre.len() + post.len()
        && name.starts_with(pre)
        && name.ends_with(post)
        && name[pre.len()..name.len() - post.len()]
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// The registry's entry for a variable: its own, else its family's.
pub fn find(name: &str) -> Option<&'static Setting> {
    REGISTRY
        .iter()
        .find(|s| s.name == name)
        .or_else(|| REGISTRY.iter().find(|s| in_family(s.name, name)))
}

/// Whether a setting's value is never shown (`ordo_env::hides_value`).
pub fn is_secret(name: &str) -> bool {
    ordo_env::hides_value(name)
}

/// One stored row of `$deploymentsetting`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRow {
    pub value: String,
    pub by: String,
    pub at: u64,
}

/// The rows `std.deployment.settings` answers, from a process's variables
/// and the stored rows of `$deploymentsetting`. A pure function: the op
/// hands it the process's own.
pub fn effective(vars: &[(String, String)], stored: &BTreeMap<String, StoredRow>) -> Vec<Value> {
    let env: BTreeMap<&str, &str> = vars
        .iter()
        .filter(|(k, _)| k.starts_with("ORDO_"))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut rows = Vec::new();
    // The settings a row has answered for.
    let mut taken: std::collections::BTreeSet<String> = Default::default();
    for entry in REGISTRY.iter().filter(|s| !s.name.contains("<NAME>")) {
        taken.insert(entry.name.to_string());
        rows.push(one(entry.name, Some(entry), &env, stored));
    }
    // What the registry lists only as a family, and what it does not list.
    let mut rest: Vec<String> = Vec::new();
    for name in env.keys() {
        // A secret's file form is a row under the secret's own name.
        let row_name = match name.strip_suffix("_FILE") {
            Some(stem) if is_secret(stem) => stem,
            _ => name,
        };
        if !rest.iter().any(|r| r == row_name) {
            rest.push(row_name.to_string());
        }
    }
    for name in rest {
        if taken.insert(name.clone()) {
            rows.push(one(&name, find(&name), &env, stored));
        }
    }
    for layer in LAYERS {
        let Some(r) = stored.get(layer.key) else {
            continue;
        };
        rows.push(json!({
            "kind": "layer",
            "name": layer.key,
            "group": LAYER_GROUP,
            "what": layer.what,
            "listed": true,
            "secret": false,
            "set": true,
            "source": "stored",
            "value": r.value,
            "storedBy": layer.op,
            "stored": {"op": layer.op, "updatedBy": r.by, "updatedAt": r.at},
        }));
    }
    rows
}

/// One setting's row.
fn one(
    name: &str,
    entry: Option<&Setting>,
    env: &BTreeMap<&str, &str>,
    stored: &BTreeMap<String, StoredRow>,
) -> Value {
    let secret = is_secret(name);
    let file_var = format!("{name}_FILE");
    let file = secret
        && env
            .get(file_var.as_str())
            .is_some_and(|v| !v.trim().is_empty());
    let plain = env.get(name).copied();
    let (source, var, value) = match (file, plain) {
        (true, _) => ("file", Some(file_var), None),
        (false, Some(v)) => ("environment", Some(name.to_string()), Some(v.to_string())),
        (false, None) => ("default", None, None),
    };
    let set = source != "default";
    let by = STORED.iter().find(|b| b.setting == name);
    let mut out = json!({
        "kind": "setting",
        "name": name,
        "group": entry.map(|e| e.group).unwrap_or(UNLISTED),
        "default": entry.map(|e| e.default),
        "range": entry.map(|e| e.range).filter(|r| !r.is_empty()),
        "family": entry.map(|e| e.name).filter(|n| n.contains("<NAME>")),
        "listed": entry.is_some(),
        "secret": secret,
        "set": set,
        "source": source,
        "var": var,
        "value": if secret { None } else { value.clone() },
        "environment": Value::Null,
        "storedBy": by.map(|b| b.op),
        "stored": Value::Null,
    });
    if let Some((b, r)) = by.and_then(|b| stored.get(b.key).map(|r| (b, r))) {
        // The stored value wins over the variable once there is one.
        out["source"] = json!("stored");
        out["set"] = json!(true);
        out["value"] = json!(r.value);
        out["environment"] = if secret {
            json!(if set { "set" } else { "unset" })
        } else {
            json!(value)
        };
        out["stored"] = json!({
            "op": b.op,
            "updatedBy": r.by,
            "updatedAt": r.at,
        });
    }
    out
}

/// The op's declaration.
pub fn specs() -> Vec<OpSpec> {
    vec![OpSpec {
        name: SETTINGS.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind: OpKind::Read,
        params: Vec::new(),
        output_fields: [
            "kind",
            "name",
            "group",
            "default",
            "range",
            "family",
            "listed",
            "secret",
            "set",
            "source",
            "var",
            "value",
            "environment",
            "storedBy",
            "stored",
            "what",
        ]
        .iter()
        .map(|f| FieldSpec::new(*f))
        .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }]
}

/// What the op needs.
pub struct SettingsOps {
    /// The control database, for `$deploymentsetting`.
    pub control: Pool,
    /// The deployment's own space, the only one the op answers in.
    pub home: String,
    /// The process's variables, taken at boot. Every `ORDO_*` one is read;
    /// a secret's value is dropped before it is held
    /// ([`SettingsOps::from_vars`]).
    pub vars: Vec<(String, String)>,
}

impl SettingsOps {
    /// Hold `vars`, with every secret's value replaced, so the op never
    /// keeps one in memory.
    pub fn from_vars(control: Pool, home: String, vars: Vec<(String, String)>) -> SettingsOps {
        SettingsOps {
            control,
            home,
            vars: held(vars),
        }
    }

    /// `std.deployment.settings`.
    pub async fn execute(
        &self,
        space: &str,
        _bound: &BoundParams,
    ) -> Result<ExecResult, ExecError> {
        if space != self.home {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "the deployment's settings are the deployment's: this op answers only in the \
                 deployment's own space",
            ));
        }
        let stored = read_stored(&self.control).await.map_err(|e| {
            ExecError::of_fault(
                e.class(),
                ErrorCode::SpaceUnavailable,
                format!(
                    "the stored settings could not be read ({e}). Nothing was read, which is not \
                     the same as nothing being stored"
                ),
            )
        })?;
        Ok(ExecResult {
            rows: effective(&self.vars, &stored),
            next_cursor: None,
            position: None,
        })
    }
}

/// The `ORDO_*` variables, each secret's value replaced by `<set>`.
pub fn held(vars: Vec<(String, String)>) -> Vec<(String, String)> {
    vars.into_iter()
        .filter(|(k, _)| k.starts_with("ORDO_"))
        .map(|(k, v)| {
            if is_secret(&k) {
                (k, "<set>".to_string())
            } else {
                (k, v)
            }
        })
        .collect()
}

/// Every row of `$deploymentsetting`.
pub async fn read_stored(pool: &Pool) -> Result<BTreeMap<String, StoredRow>, PoolError> {
    pool.rows(crate::admission::SETTING_DDL).await?;
    let rows = pool
        .rows(r#"SELECT "key", "value", "updated_by", "updated_at" FROM "$deploymentsetting""#)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let key = r.first().cloned().flatten()?;
            Some((
                key,
                StoredRow {
                    value: r.get(1).cloned().flatten().unwrap_or_default(),
                    by: r.get(2).cloned().flatten().unwrap_or_default(),
                    at: r
                        .get(3)
                        .cloned()
                        .flatten()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                },
            ))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The sections of `docs/settings.md` that hold no deployment setting.
    const NOT_SETTINGS: [&str; 3] = ["Settings that disagree", "Not traced", "How to read"];
    const DEV: &str = "14 ";

    /// The names in a row's first cell: each backticked `ORDO_*` name, and
    /// `_X` after a family (`ORDO_AUTH_<NAME>_AUTHORIZE_URL`, `_TOKEN_URL`).
    fn names_of(cell: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut prefix: Option<String> = None;
        for (i, token) in cell.split('`').enumerate() {
            if i % 2 == 0 {
                continue;
            }
            let left = token.split('=').next().unwrap_or("");
            let ok = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || "_<>".contains(c);
            if left.is_empty() || !left.chars().all(ok) {
                continue;
            }
            if left.starts_with("ORDO_") {
                out.push(left.to_string());
                prefix = left.find("<NAME>").map(|at| left[..at + 6].to_string());
            } else if left.starts_with('_') && token == left {
                if let Some(p) = &prefix {
                    out.push(format!("{p}{left}"));
                }
            }
        }
        out
    }

    /// The name a doc row's variable is registered under: a secret's
    /// `_FILE` form is the secret itself.
    fn registered_as(name: &str) -> String {
        if PATH_SETTINGS.contains(&name) {
            return name.to_string();
        }
        match name.strip_suffix("_FILE") {
            Some(stem) if is_secret(stem) => stem.to_string(),
            _ => name.to_string(),
        }
    }

    /// Settings whose own name ends in `_FILE`: a path, with no value form
    /// beside it, so they are registered under that name.
    const PATH_SETTINGS: [&str; 1] = ["ORDO_SUPERVISOR_KEY_FILE"];

    /// Review, step 7: the registry once said `ORDO_SUPERVISOR_KEY` for
    /// `ORDO_SUPERVISOR_KEY_FILE`. The supervisor's crate is skipped by the
    /// scan below, so every `ORDO_SUPERVISOR_*` name its settings module
    /// reads is checked here, by its exact name.
    #[test]
    fn every_supervisor_setting_is_registered_by_its_own_name() {
        let text = std::fs::read_to_string(repo().join("crates/ordo-supervisor/src/config.rs"))
            .expect("the supervisor's config.rs");
        let mut names = BTreeSet::new();
        // '\x22' is a double quote, written so this file's own quotes stay
        // paired for the scan below.
        for (i, piece) in text.split('\x22').enumerate() {
            if i % 2 == 1
                && piece.starts_with(concat!("ORDO_", "SUPERVISOR_"))
                // Set by the supervisor for its launcher, not by anyone.
                && !piece.ends_with("_LAUNCH_USER")
                && !piece.ends_with("_LAUNCH_RELAY")
                && piece
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                names.insert(piece.to_string());
            }
        }
        assert!(names.len() >= 10, "found only {names:?}");
        for n in &names {
            assert!(
                REGISTRY.iter().any(|s| s.name == n),
                "{n} is read by ordo-supervisor and not registered by that name"
            );
        }
    }

    /// (section, names, default, built) for every settings row of the doc.
    fn doc_rows() -> Vec<(String, Vec<String>, String, String)> {
        let text = std::fs::read_to_string(repo().join("docs/settings.md")).expect("settings.md");
        let mut section = String::new();
        let mut out = Vec::new();
        for line in text.lines() {
            if let Some(h) = line.strip_prefix("## ") {
                section = h.trim().to_string();
                continue;
            }
            if section.is_empty() || !line.starts_with("| ") {
                continue;
            }
            let cells: Vec<&str> = line
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            if cells.len() < 4 {
                continue;
            }
            let names = names_of(cells[0]);
            if names.is_empty() {
                continue;
            }
            out.push((
                section.clone(),
                names,
                cells[2].to_string(),
                cells[3].to_string(),
            ));
        }
        out
    }

    fn is_deployment_row(section: &str, built: &str) -> bool {
        !NOT_SETTINGS.iter().any(|s| section.starts_with(s))
            && !section.starts_with(DEV)
            && !["withdrawn", "planned", "not built"]
                .iter()
                .any(|w| built.starts_with(w))
    }

    /// `docs/settings.md` and [`REGISTRY`] name the same settings with the
    /// same defaults. Add a setting to both, or neither.
    #[test]
    fn the_registry_and_the_settings_doc_agree() {
        let rows = doc_rows();
        let mut doc: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (section, names, default, built) in &rows {
            if !is_deployment_row(section, built) {
                continue;
            }
            for n in names {
                doc.entry(registered_as(n))
                    .or_default()
                    .push(default.clone());
            }
        }
        let mut seen = BTreeSet::new();
        for s in REGISTRY {
            assert!(seen.insert(s.name), "{} is in the registry twice", s.name);
            let Some(defaults) = doc.get(s.name) else {
                panic!(
                    "{} is in the registry and not in docs/settings.md: add its row, or remove it",
                    s.name
                );
            };
            assert!(
                defaults.iter().any(|d| d == s.default),
                "{}: the registry's default `{}` is not the doc's ({defaults:?})",
                s.name,
                s.default
            );
        }
        for name in doc.keys() {
            assert!(
                seen.contains(name.as_str()),
                "{name} is in docs/settings.md and not in the registry \
                 (crates/ordo-host/src/settings.rs): add it"
            );
        }
    }

    /// Names a crate reads that are not deployment settings: the console's
    /// own, test names, and a name read only to refuse it.
    const NOT_DEPLOYMENT: [&str; 4] = [
        "ORDO_",
        "ORDO_A",
        "ORDO_B",
        // Read only to refuse it, naming ORDO_MAIL_CREDENTIALS_FILE.
        "ORDO_MAIL_PASSWORD",
    ];
    /// Crates that read no deployment setting of `ordo-host`'s.
    /// Programs with their own settings, documented with them: the identity
    /// service's are in `docs/howto-identity.md`, section 13.
    /// The supervisor reads every child's settings only to hand each its
    /// own; its own `ORDO_SUPERVISOR_*` are registered above.
    const OTHER_CRATES: [&str; 6] = [
        "ordo-console",
        "ordo-cli",
        "ordo-env",
        "ordo-conformance",
        "ordo-identity-service",
        "ordo-supervisor",
    ];

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                rust_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// Every `"ORDO_*"` name the deployment's crates read is registered,
    /// or is a development tool's (section 14 of the doc).
    #[test]
    fn every_setting_the_code_reads_is_registered() {
        let dev: BTreeSet<String> = doc_rows()
            .into_iter()
            .filter(|(section, ..)| section.starts_with(DEV))
            .flat_map(|(_, names, ..)| names)
            .collect();
        let mut files = Vec::new();
        for e in std::fs::read_dir(repo().join("crates"))
            .expect("crates")
            .flatten()
        {
            let name = e.file_name().to_string_lossy().to_string();
            if !OTHER_CRATES.contains(&name.as_str()) {
                rust_files(&e.path().join("src"), &mut files);
            }
        }
        assert!(files.len() > 50, "found only {} files", files.len());
        let mut missing = BTreeSet::new();
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap_or_default();
            for (i, piece) in text.split('"').enumerate() {
                if i % 2 == 0 || !piece.starts_with("ORDO_") {
                    continue;
                }
                if !piece
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                {
                    continue;
                }
                let name = registered_as(piece);
                if NOT_DEPLOYMENT.contains(&piece)
                    || piece.starts_with("ORDO_CONSOLE_")
                    || dev.contains(piece)
                    || find(&name).is_some()
                {
                    continue;
                }
                missing.insert(format!("{piece} ({})", f.display()));
            }
        }
        assert!(
            missing.is_empty(),
            "read by the code and not in the registry or docs/settings.md: {missing:#?}"
        );
    }

    fn vars(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn row<'a>(rows: &'a [Value], name: &str) -> &'a Value {
        rows.iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("no row {name}"))
    }

    #[test]
    fn each_row_says_its_source_and_a_secret_only_whether_it_is_set() {
        let rows = effective(
            &vars(&[
                ("ORDO_PG_PORT", "6543"),
                ("ORDO_TOKEN_KEY_FILE", "/run/secrets/token"),
                ("ORDO_PAGE_KEY", "abcdef"),
                ("ORDO_AUTH_CORP_CLIENT_ID", "corp-app"),
                ("ORDO_AUTH_CORP_CLIENT_SECRET_FILE", "/run/secrets/corp"),
                ("ORDO_SOMETHING_NEW", "x"),
                ("HOME", "/root"),
            ]),
            &BTreeMap::new(),
        );
        let port = row(&rows, "ORDO_PG_PORT");
        assert_eq!(port["source"], "environment");
        assert_eq!(port["value"], "6543");
        assert_eq!(port["default"], "5432");
        let idle = row(&rows, "ORDO_PG_IDLE_SECS");
        assert_eq!(idle["source"], "default");
        assert_eq!(idle["set"], false);
        let token = row(&rows, "ORDO_TOKEN_KEY");
        assert_eq!(token["source"], "file");
        assert_eq!(token["var"], "ORDO_TOKEN_KEY_FILE");
        assert_eq!(token["set"], true);
        assert!(token["value"].is_null());
        let page = row(&rows, "ORDO_PAGE_KEY");
        assert_eq!(page["source"], "environment");
        assert!(page["value"].is_null(), "{page}");
        let corp = row(&rows, "ORDO_AUTH_CORP_CLIENT_ID");
        assert_eq!(corp["family"], "ORDO_AUTH_<NAME>_CLIENT_ID");
        assert_eq!(corp["value"], "corp-app");
        let secret = row(&rows, "ORDO_AUTH_CORP_CLIENT_SECRET");
        assert_eq!(secret["source"], "file");
        assert!(secret["value"].is_null());
        let new = row(&rows, "ORDO_SOMETHING_NEW");
        assert_eq!(new["listed"], false);
        assert_eq!(new["group"], UNLISTED);
        let text = serde_json::to_string(&rows).unwrap();
        for leaked in ["/run/secrets", "abcdef", "/root"] {
            assert!(!text.contains(leaked), "{leaked} is in the answer");
        }
        assert!(!rows.iter().any(|r| r["name"] == "ORDO_TOKEN_KEY_FILE"));
    }

    #[test]
    fn a_stored_setting_wins_and_the_removed_claim_codes_row_is_never_shown() {
        let mut stored = BTreeMap::new();
        // The claim code was removed (owner, 2026-10-30); its row stays in
        // the database until the drop migration, and is never shown.
        stored.insert(
            "first_comer_claim_code".to_string(),
            StoredRow {
                value: "required:0123456789abcdef".into(),
                by: "environment".into(),
                at: 5,
            },
        );
        stored.insert(
            crate::auth::guest::KEY.to_string(),
            StoredRow {
                value: "off".into(),
                by: "owner".into(),
                at: 6,
            },
        );
        stored.insert(
            crate::session_policy::APP_KEY.to_string(),
            StoredRow {
                value: r#"{"tokenSecs":600}"#.into(),
                by: "owner".into(),
                at: 7,
            },
        );
        let rows = effective(
            &vars(&[
                ("ORDO_AUTH_CLAIM_CODE_FILE", "/run/claim"),
                ("ORDO_AUTH_GUEST", "on"),
            ]),
            &stored,
        );
        assert!(!rows
            .iter()
            .any(|r| r["name"] == "ORDO_AUTH_CLAIM_CODE" && r["source"] == "stored"));
        let guest = row(&rows, "ORDO_AUTH_GUEST");
        assert_eq!(guest["source"], "stored");
        assert_eq!(guest["value"], "off");
        assert_eq!(guest["environment"], "on");
        let layer = row(&rows, "session_policy");
        assert_eq!(layer["kind"], "layer");
        assert_eq!(layer["storedBy"], "std.deployment.setSessionPolicy");
        let text = serde_json::to_string(&rows).unwrap();
        assert!(!text.contains("0123456789abcdef") && !text.contains("/run/claim"));
    }

    #[test]
    fn the_op_holds_no_secret_value() {
        let kept = held(vars(&[
            ("ORDO_PAGE_KEY", "abcdef"),
            ("ORDO_PG_DB", "app"),
            ("PATH", "/bin"),
        ]));
        assert_eq!(
            kept,
            vars(&[("ORDO_PAGE_KEY", "<set>"), ("ORDO_PG_DB", "app")])
        );
    }

    #[test]
    fn a_family_takes_no_setting_of_its_own() {
        assert_eq!(
            find("ORDO_AUTH_OIDC_CACHE_SECS").unwrap().name,
            "ORDO_AUTH_OIDC_CACHE_SECS"
        );
        assert_eq!(
            find("ORDO_AUTH_GH_PRESET").unwrap().name,
            "ORDO_AUTH_<NAME>_PRESET"
        );
        assert!(find("ORDO_AUTH_MY_CORP_PRESET").is_none());
        assert_eq!(
            find("ORDO_WORKER_ATTEST_TICK_MS").unwrap().name,
            "ORDO_WORKER_<NAME>_TICK_MS"
        );
    }
}
