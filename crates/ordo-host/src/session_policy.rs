//! How long a session lasts, in three layers (owner, 2026-10-21: "there
//! should however be session settings the app dev can set", and tenants
//! may want their own "because different companies different compliance").
//!
//! | Layer | Sets | How |
//! |---|---|---|
//! | Deployment | the defaults and the absolute limits | `ORDO_SESSION_SECS`, `ORDO_SESSION_MAX_SECS`, `ORDO_REMEMBER`, `ORDO_REMEMBER_SECS`, and the `*_LIMIT` settings |
//! | App developer | its own defaults, and optionally a narrower range per value | `std.deployment.setSessionPolicy` |
//! | Tenant | its own values, inside the app developer's range, or inside the deployment's limits when the app developer set no range | `std.space.setSessionPolicy`, read with `std.space.sessionPolicy` |
//!
//! A tenant may choose longer or shorter than the app developer's default
//! (owner: "It is their space"). The effective value for a session is the
//! tenant's, else the app developer's, else the deployment's. It is resolved
//! at sign-in and again at each refresh, from the tenant the session is for,
//! so a tenant's change reaches its sessions at their next refresh.
//!
//! Whether a tenant may set its own at all is the app developer's, through
//! the access template that already governs who may call an op
//! (`ORDO_ACCESS_TEMPLATE="std.space.setSessionPolicy="` closes it to every
//! role), not through a second switch here.
//!
//! `ORDO_REMEMBER=off` is absolute: no layer can turn renewals back on.
//!
//! # The overall limit (owner, 2026-10-22)
//!
//! `rememberMaxSecs`: N seconds after the original sign-in, the person signs
//! in again, however often the sign-in was used in between. The default is
//! none. It is layered like the idle lifetime: `ORDO_REMEMBER_MAX_SECS` at the
//! deployment, the app developer's value and optional range, and a tenant's
//! own. `ORDO_REMEMBER_MAX_SECS_LIMIT` is the absolute cap: with it set, no
//! layer may choose none or longer, and a sign-in with nothing chosen ends at
//! the cap. A layer writes `none` for no limit, `default` to clear its value.
//! The original sign-in time travels with the renewal family through every
//! rotation (`renewal::Family::signed_in_at`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::{sql_lit, Pool, PoolError};
use serde_json::{json, Value};

/// `std.deployment.setSessionPolicy`: the app developer's layer.
pub const SET_APP: &str = "std.deployment.setSessionPolicy";
/// `std.space.setSessionPolicy`: a tenant's own values.
pub const SET_TENANT: &str = "std.space.setSessionPolicy";
/// `std.space.sessionPolicy`: what is in force in a tenant, and why.
pub const READ_TENANT: &str = "std.space.sessionPolicy";
/// Every op of this module.
pub const OPS: [&str; 3] = [SET_APP, SET_TENANT, READ_TENANT];
/// The ops the app developer's access template may name: a tenant's setter
/// and its read. Everything else about them is the platform's.
/// A tenant's link password limits are governed the same way
/// ([`crate::link_password`]).
/// So is a tenant's password slowdown ([`crate::guess_limits`]).
pub const TEMPLATE_GOVERNED: [&str; 4] = [
    SET_TENANT,
    READ_TENANT,
    crate::share_links::SET_PASSWORD_LIMITS,
    crate::guess_limits::SET_TENANT,
];

/// The tenant setters among them: the template may close each to tenants,
/// and the owner still reaches the op so the refusal names the setting.
pub const TENANT_SETTERS: [&str; 3] = [
    SET_TENANT,
    crate::share_links::SET_PASSWORD_LIMITS,
    crate::guess_limits::SET_TENANT,
];

/// Why `std.space.setSessionPolicy` is refused to a caller of `role`, when
/// the app developer's access template closed it to that role. The door lets
/// a tenant owner through to the op so the refusal can name the setting that
/// allows it; every other role the template leaves out is refused at the door.
pub fn closed_by_template(
    template: &crate::access_template::AccessTemplate,
    role: Option<ordo_identity::principal::Role>,
) -> Option<String> {
    closed_by_template_for(SET_TENANT, template, role)
}

/// [`closed_by_template`] for any of [`TENANT_SETTERS`].
pub fn closed_by_template_for(
    op: &str,
    template: &crate::access_template::AccessTemplate,
    role: Option<ordo_identity::principal::Role>,
) -> Option<String> {
    let roles = template.roles_for(op)?;
    if role.is_some_and(|r| roles.contains(&r)) {
        return None;
    }
    Some(format!(
        "the app developer has closed {op} to this tenant's roles: the deployment owner \
         allows it with {}=\"{op}=owner\"",
        crate::access_template::ENV
    ))
}

pub const TOKEN_LIMIT_ENV: &str = "ORDO_SESSION_SECS_LIMIT";
pub const MAX_LIMIT_ENV: &str = "ORDO_SESSION_MAX_SECS_LIMIT";
pub const REMEMBER_LIMIT_ENV: &str = "ORDO_REMEMBER_SECS_LIMIT";
/// The deployment's overall limit on a remembered sign-in. Unset is none.
pub const REMEMBER_MAX_ENV: &str = "ORDO_REMEMBER_MAX_SECS";
/// The absolute cap on the overall limit. Unset is no cap.
pub const REMEMBER_MAX_LIMIT_ENV: &str = "ORDO_REMEMBER_MAX_SECS_LIMIT";

/// The shortest token any layer may ask for.
pub const TOKEN_FLOOR: u64 = 60;
/// A day: a token's claims are frozen at mint, so its life is how long a
/// stale claim can live.
pub const DEFAULT_TOKEN_LIMIT: u64 = 86_400;
/// 30 days.
pub const DEFAULT_MAX_LIMIT: u64 = 30 * 86_400;
/// 400 days, the longest a browser keeps a cookie.
pub const DEFAULT_REMEMBER_LIMIT: u64 = 400 * 86_400;

pub(crate) const APP_KEY: &str = "session_policy";

/// One value per field; `None` is "not set at this layer".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Values {
    pub token_secs: Option<u64>,
    pub max_secs: Option<u64>,
    pub remember: Option<bool>,
    pub remember_secs: Option<u64>,
    /// The overall limit. `Some(0)` is "none" chosen at this layer.
    pub remember_max_secs: Option<u64>,
}

/// A closed range, inclusive.
pub type Range = (u64, u64);

/// The app developer's ranges. `None` leaves the deployment's limits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ranges {
    pub token_secs: Option<Range>,
    pub max_secs: Option<Range>,
    pub remember_secs: Option<Range>,
    pub remember_max_secs: Option<Range>,
}

/// The app developer's layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppPolicy {
    pub values: Values,
    pub ranges: Ranges,
}

/// The deployment's layer: the defaults and the absolute limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deployment {
    pub token_secs: u64,
    pub max_secs: u64,
    pub remember: bool,
    pub remember_secs: u64,
    pub token_limit: u64,
    pub max_limit: u64,
    pub remember_limit: u64,
    /// The overall limit on a remembered sign-in. `None` is none.
    pub remember_max_secs: Option<u64>,
    /// The absolute cap on the overall limit. `None` is no cap.
    pub remember_max_limit: Option<u64>,
}

impl Deployment {
    /// The deployment layer from the four session settings already read,
    /// and the three limits from `env`. A default above its limit refuses
    /// the boot, naming both.
    pub fn from_vars(
        env: &dyn Fn(&str) -> Option<String>,
        token_secs: u64,
        max_secs: u64,
        remember: crate::renewal::RememberConfig,
    ) -> Result<Deployment, (&'static str, String)> {
        let limit = |var: &'static str, default: u64| -> Result<u64, (&'static str, String)> {
            match env(var).as_deref().map(str::trim) {
                None | Some("") => Ok(default),
                Some(v) => v.parse::<u64>().ok().filter(|n| *n > 0).ok_or((
                    var,
                    format!("`{v}` is not a number of seconds above zero (default {default})"),
                )),
            }
        };
        let optional = |var: &'static str| -> Result<Option<u64>, (&'static str, String)> {
            match env(var).as_deref().map(str::trim) {
                None | Some("") | Some("none") => Ok(None),
                Some(v) => v
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n >= TOKEN_FLOOR)
                    .map(Some)
                    .ok_or((
                        var,
                        format!(
                            "`{v}` is not a number of seconds of {TOKEN_FLOOR} or more; unset \
                             it or write none for no limit"
                        ),
                    )),
            }
        };
        let d = Deployment {
            token_secs,
            max_secs,
            remember: remember.on,
            remember_secs: remember.idle_secs,
            token_limit: limit(TOKEN_LIMIT_ENV, DEFAULT_TOKEN_LIMIT)?,
            max_limit: limit(MAX_LIMIT_ENV, DEFAULT_MAX_LIMIT)?,
            remember_limit: limit(REMEMBER_LIMIT_ENV, DEFAULT_REMEMBER_LIMIT)?,
            remember_max_secs: optional(REMEMBER_MAX_ENV)?,
            remember_max_limit: optional(REMEMBER_MAX_LIMIT_ENV)?,
        };
        let over = |var: &'static str, v: u64, lim_var: &str, lim: u64| {
            Err((
                var,
                format!("{v} is above {lim_var}={lim}; raise the limit or lower this"),
            ))
        };
        if d.token_secs > d.token_limit {
            return over(
                "ORDO_SESSION_SECS",
                d.token_secs,
                TOKEN_LIMIT_ENV,
                d.token_limit,
            );
        }
        if d.max_secs > d.max_limit {
            return over(
                "ORDO_SESSION_MAX_SECS",
                d.max_secs,
                MAX_LIMIT_ENV,
                d.max_limit,
            );
        }
        if d.remember_secs > d.remember_limit {
            return over(
                crate::renewal::SECS_SETTING,
                d.remember_secs,
                REMEMBER_LIMIT_ENV,
                d.remember_limit,
            );
        }
        if let Some(lim) = d.remember_max_limit {
            match d.remember_max_secs {
                Some(v) if v > lim => {
                    return over(REMEMBER_MAX_ENV, v, REMEMBER_MAX_LIMIT_ENV, lim);
                }
                _ => {}
            }
        }
        Ok(d)
    }

    /// The same deployment with an overall limit and its cap. For tests.
    pub fn with_remember_max(mut self, secs: Option<u64>, limit: Option<u64>) -> Deployment {
        self.remember_max_secs = secs;
        self.remember_max_limit = limit;
        self
    }

    /// The defaults alone, with the default limits. For tests and for an
    /// assembly that read no limits.
    pub fn with_defaults(
        token_secs: u64,
        max_secs: u64,
        remember: crate::renewal::RememberConfig,
    ) -> Deployment {
        Deployment {
            token_secs,
            max_secs,
            remember: remember.on,
            remember_secs: remember.idle_secs,
            token_limit: DEFAULT_TOKEN_LIMIT.max(token_secs),
            max_limit: DEFAULT_MAX_LIMIT.max(max_secs),
            remember_limit: DEFAULT_REMEMBER_LIMIT.max(remember.idle_secs),
            remember_max_secs: None,
            remember_max_limit: None,
        }
    }

    fn limit(&self, f: Field) -> Range {
        match f {
            Field::Token => (TOKEN_FLOOR, self.token_limit),
            Field::Max => (TOKEN_FLOOR, self.max_limit),
            Field::RememberSecs => (TOKEN_FLOOR, self.remember_limit),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Token,
    Max,
    RememberSecs,
}

impl Field {
    fn param(self) -> &'static str {
        match self {
            Field::Token => "tokenSecs",
            Field::Max => "maxSecs",
            Field::RememberSecs => "rememberSecs",
        }
    }
    fn limit_env(self) -> &'static str {
        match self {
            Field::Token => TOKEN_LIMIT_ENV,
            Field::Max => MAX_LIMIT_ENV,
            Field::RememberSecs => REMEMBER_LIMIT_ENV,
        }
    }
    fn of(self, v: &Values) -> Option<u64> {
        match self {
            Field::Token => v.token_secs,
            Field::Max => v.max_secs,
            Field::RememberSecs => v.remember_secs,
        }
    }
    fn range(self, r: &Ranges) -> Option<Range> {
        match self {
            Field::Token => r.token_secs,
            Field::Max => r.max_secs,
            Field::RememberSecs => r.remember_secs,
        }
    }
}

const FIELDS: [Field; 3] = [Field::Token, Field::Max, Field::RememberSecs];

/// Which layer a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Deployment,
    App,
    Tenant,
}

impl Source {
    pub fn word(self) -> &'static str {
        match self {
            Source::Deployment => "deployment",
            Source::App => "app",
            Source::Tenant => "tenant",
        }
    }
}

/// What is in force for one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub token_secs: u64,
    pub max_secs: u64,
    pub remember: bool,
    pub remember_secs: u64,
    /// The overall limit: the sign-in ends this long after it was made,
    /// however often it was renewed. `None` is none.
    pub remember_max_secs: Option<u64>,
    /// Where each value came from: token, ceiling, remember, idle, overall.
    pub sources: [Source; 5],
}

impl Effective {
    /// How long one session may be refreshed for: the ceiling, or the overall
    /// limit when that is shorter.
    pub fn ceiling_secs(&self) -> u64 {
        self.remember_max_secs
            .map_or(self.max_secs, |m| m.min(self.max_secs))
    }

    /// When a sign-in made at `signed_in_at` must be made again, if ever.
    pub fn overall_end(&self, signed_in_at: u64) -> Option<u64> {
        self.remember_max_secs
            .map(|m| signed_in_at.saturating_add(m))
    }
}

/// The range a tenant's overall limit must sit in, and who set it. `None` is
/// no range: any value, or none.
fn overall_range(d: &Deployment, app: &AppPolicy) -> Option<(Range, String)> {
    if let Some(r) = app.ranges.remember_max_secs {
        return Some((
            r,
            format!("the range the app developer set ({SET_APP} rememberMaxSecsRange)"),
        ));
    }
    d.remember_max_limit.map(|lim| {
        (
            (TOKEN_FLOOR, lim),
            format!("the deployment's limit ({REMEMBER_MAX_LIMIT_ENV})"),
        )
    })
}

/// Is `v` (0 for none) a value inside `range`, or allowed with no range?
fn overall_inside(v: u64, range: Option<Range>) -> bool {
    match range {
        Some((lo, hi)) => v != 0 && v >= lo && v <= hi,
        None => v == 0 || v >= TOKEN_FLOOR,
    }
}

/// The overall limit in force, and where it came from. The deployment's cap
/// is absolute: nothing chosen, or none, ends at the cap.
fn resolve_overall(d: &Deployment, app: &AppPolicy, tenant: &Values) -> (Option<u64>, Source) {
    let dep_range = d.remember_max_limit.map(|l| (TOKEN_FLOOR, l));
    let tenant_range = overall_range(d, app).map(|(r, _)| r);
    let (chosen, source) = if let Some(v) = tenant
        .remember_max_secs
        .filter(|v| overall_inside(*v, tenant_range))
    {
        (v, Source::Tenant)
    } else if let Some(v) = app
        .values
        .remember_max_secs
        .filter(|v| overall_inside(*v, dep_range))
    {
        (v, Source::App)
    } else {
        (d.remember_max_secs.unwrap_or(0), Source::Deployment)
    };
    let chosen = (chosen != 0).then_some(chosen);
    match d.remember_max_limit {
        Some(lim) => (Some(chosen.map_or(lim, |v| v.min(lim))), source),
        None => (chosen, source),
    }
}

/// The range a tenant's value must sit in for `f`, and who set it.
fn tenant_range(d: &Deployment, app: &AppPolicy, f: Field) -> (Range, String) {
    match f.range(&app.ranges) {
        Some(r) => (
            r,
            format!(
                "the range the app developer set ({SET_APP} {}Range)",
                f.param()
            ),
        ),
        None => (
            d.limit(f),
            format!("the deployment's limit ({})", f.limit_env()),
        ),
    }
}

/// Resolve the three layers. A stored value outside the range in force now
/// (the app developer narrowed it later) is passed over, never clamped: the
/// next layer down answers, and the read op says so.
pub fn resolve(d: &Deployment, app: &AppPolicy, tenant: &Values) -> Effective {
    let pick = |f: Field, base: u64| -> (u64, Source) {
        let (range, _) = tenant_range(d, app, f);
        let inside = |v: u64| v >= range.0 && v <= range.1;
        let app_ok = |v: u64| {
            let lim = d.limit(f);
            v >= lim.0 && v <= lim.1
        };
        if let Some(v) = f.of(tenant).filter(|v| inside(*v)) {
            return (v, Source::Tenant);
        }
        if let Some(v) = f.of(&app.values).filter(|v| app_ok(*v)) {
            return (v, Source::App);
        }
        (base, Source::Deployment)
    };
    let (token, ts) = pick(Field::Token, d.token_secs);
    let (max, ms) = pick(Field::Max, d.max_secs);
    let (rsecs, rss) = pick(Field::RememberSecs, d.remember_secs);
    let (remember, rs) = if !d.remember {
        (false, Source::Deployment)
    } else if let Some(r) = tenant.remember {
        (r, Source::Tenant)
    } else if let Some(r) = app.values.remember {
        (r, Source::App)
    } else {
        (true, Source::Deployment)
    };
    let (overall, os) = resolve_overall(d, app, tenant);
    Effective {
        // A token never outlives the session it belongs to.
        token_secs: token.min(max),
        max_secs: max,
        remember,
        remember_secs: rsecs,
        remember_max_secs: overall,
        sources: [ts, ms, rs, rss, os],
    }
}

fn refused(why: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, why.into())
}

fn unavailable(e: impl crate::Faulty) -> ExecError {
    crate::op_store_unavailable(e)
}

/// A parameter: absent, `default` (clear this layer's value), or a value.
enum Change<T> {
    Keep,
    Clear,
    Set(T),
}

fn param<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn secs_change(bound: &BoundParams, name: &str) -> Result<Change<u64>, ExecError> {
    match param(bound, name) {
        None => Ok(Change::Keep),
        Some("default") => Ok(Change::Clear),
        Some(v) => v.parse::<u64>().map(Change::Set).map_err(|_| {
            refused(format!(
                "{name}: `{v}` is not a number of seconds; write a number, or `default` to \
                 use the next layer's value"
            ))
        }),
    }
}

/// `rememberMaxSecs`: a number of seconds, `none` (stored as 0), or
/// `default`.
fn overall_change(bound: &BoundParams) -> Result<Change<u64>, ExecError> {
    match param(bound, "rememberMaxSecs") {
        None => Ok(Change::Keep),
        Some("default") => Ok(Change::Clear),
        Some("none") => Ok(Change::Set(0)),
        Some(v) => v.parse::<u64>().map(Change::Set).map_err(|_| {
            refused(format!(
                "rememberMaxSecs: `{v}` is not a number of seconds; write a number, `none` for \
                 no overall limit, or `default` to use the next layer's value"
            ))
        }),
    }
}

/// Check an overall limit (0 is none) against `range`, naming who set it.
fn overall_check(v: u64, range: Option<(Range, String)>) -> Result<(), ExecError> {
    let bare = range.as_ref().map(|(r, _)| *r);
    if overall_inside(v, bare) {
        return Ok(());
    }
    let shown = if v == 0 {
        "none".to_string()
    } else {
        v.to_string()
    };
    Err(refused(match range {
        Some(((lo, hi), why)) => format!("rememberMaxSecs {shown} is outside {lo}-{hi}, {why}"),
        None => format!("rememberMaxSecs {shown} is below the floor of {TOKEN_FLOOR} seconds"),
    }))
}

fn remember_change(bound: &BoundParams) -> Result<Change<bool>, ExecError> {
    match param(bound, "remember") {
        None => Ok(Change::Keep),
        Some("default") => Ok(Change::Clear),
        Some("on") => Ok(Change::Set(true)),
        Some("off") => Ok(Change::Set(false)),
        Some(v) => Err(refused(format!(
            "remember: `{v}` is not on, off or default"
        ))),
    }
}

fn range_change(bound: &BoundParams, name: &str) -> Result<Change<Range>, ExecError> {
    match param(bound, name) {
        None => Ok(Change::Keep),
        Some("default") | Some("none") => Ok(Change::Clear),
        Some(v) => {
            let parsed = v
                .split_once('-')
                .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)));
            match parsed {
                Some((a, b)) if a <= b => Ok(Change::Set((a, b))),
                _ => Err(refused(format!(
                    "{name}: `{v}` is not a range; write min-max in seconds (300-7200), or \
                     `none` to leave tenants the deployment's limits"
                ))),
            }
        }
    }
}

fn apply<T: Copy>(slot: &mut Option<T>, c: Change<T>) {
    match c {
        Change::Keep => {}
        Change::Clear => *slot = None,
        Change::Set(v) => *slot = Some(v),
    }
}

fn values_json(v: &Values) -> Value {
    json!({
        "tokenSecs": v.token_secs,
        "maxSecs": v.max_secs,
        "remember": v.remember.map(|r| if r { "on" } else { "off" }),
        "rememberSecs": v.remember_secs,
        "rememberMaxSecs": overall_json(v.remember_max_secs),
    })
}

/// A stored or chosen overall limit: a number, `"none"` for 0, null unset.
fn overall_json(v: Option<u64>) -> Value {
    match v {
        None => Value::Null,
        Some(0) => json!("none"),
        Some(n) => json!(n),
    }
}

fn values_from(v: &Value) -> Values {
    let n = |k: &str| v.get(k).and_then(Value::as_u64);
    Values {
        token_secs: n("tokenSecs"),
        max_secs: n("maxSecs"),
        remember: v
            .get("remember")
            .and_then(Value::as_str)
            .and_then(|s| match s {
                "on" => Some(true),
                "off" => Some(false),
                _ => None,
            }),
        remember_secs: n("rememberSecs"),
        remember_max_secs: match v.get("rememberMaxSecs") {
            Some(Value::String(s)) if s == "none" => Some(0),
            other => other.and_then(Value::as_u64),
        },
    }
}

fn range_json(r: Option<Range>) -> Value {
    r.map(|(a, b)| json!(format!("{a}-{b}")))
        .unwrap_or(Value::Null)
}

fn range_from(v: Option<&Value>) -> Option<Range> {
    let (a, b) = v?.as_str()?.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn app_json(a: &AppPolicy) -> Value {
    let mut v = values_json(&a.values);
    v["tokenSecsRange"] = range_json(a.ranges.token_secs);
    v["maxSecsRange"] = range_json(a.ranges.max_secs);
    v["rememberSecsRange"] = range_json(a.ranges.remember_secs);
    v["rememberMaxSecsRange"] = range_json(a.ranges.remember_max_secs);
    v
}

fn app_from(v: &Value) -> AppPolicy {
    AppPolicy {
        values: values_from(v),
        ranges: Ranges {
            token_secs: range_from(v.get("tokenSecsRange")),
            max_secs: range_from(v.get("maxSecsRange")),
            remember_secs: range_from(v.get("rememberSecsRange")),
            remember_max_secs: range_from(v.get("rememberMaxSecsRange")),
        },
    }
}

/// The two tables this module adds. `$deploymentsetting` holds the app
/// developer's layer.
pub const TENANT_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$sessionpolicy" (
  "space"      text   NOT NULL,
  "policy"     text   NOT NULL,
  "updated_by" text   NOT NULL DEFAULT '',
  "updated_at" bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("space")
)"#;

/// Which tenant each session is for, so a refresh resolves the right
/// tenant's values. Kept until the session's ceiling, then swept.
pub const SESSION_SPACE_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$sessionspace" (
  "session_id" text   NOT NULL,
  "space"      text   NOT NULL,
  "until"      bigint NOT NULL,
  PRIMARY KEY ("session_id")
)"#;

pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [crate::admission::SETTING_DDL, TENANT_DDL, SESSION_SPACE_DDL] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct Memory {
    app: AppPolicy,
    tenants: BTreeMap<String, Values>,
    sessions: BTreeMap<String, (String, u64)>,
}

enum Backing {
    Pg(Pool),
    Memory(Mutex<Memory>),
}

/// The three layers, and where the two stored ones live.
pub struct SessionPolicies {
    pub deployment: Deployment,
    backing: Backing,
    /// The deployment's own space: the app developer's op answers there only.
    pub home: Option<String>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
}

impl SessionPolicies {
    pub fn on_pool(deployment: Deployment, pool: Pool) -> SessionPolicies {
        SessionPolicies {
            deployment,
            backing: Backing::Pg(pool),
            home: None,
            now: Arc::new(crate::runtime::now_secs),
            stream: None,
        }
    }

    /// Held in memory: tests.
    pub fn in_memory(deployment: Deployment) -> SessionPolicies {
        SessionPolicies {
            deployment,
            backing: Backing::Memory(Mutex::new(Memory::default())),
            home: None,
            now: Arc::new(crate::runtime::now_secs),
            stream: None,
        }
    }

    pub fn with_home(mut self, home: impl Into<String>) -> SessionPolicies {
        self.home = Some(home.into());
        self
    }

    pub fn with_stream(mut self, stream: Arc<ordo_secevent::Recorder>) -> SessionPolicies {
        self.stream = Some(stream);
        self
    }

    pub async fn app(&self) -> Result<AppPolicy, ordo_identity::StoreFault> {
        match &self.backing {
            Backing::Memory(m) => Ok(m.lock().expect("policies").app),
            Backing::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "value" FROM "$deploymentsetting" WHERE "key" = {}"#,
                        sql_lit(APP_KEY)
                    ))
                    .await
                    .map_err(|e| {
                        crate::auth::store_failure::pool_fault(
                            e,
                            "the app developer's session policy could not be read",
                        )
                    })?;
                Ok(rows
                    .first()
                    .and_then(|r| r.first().cloned().flatten())
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .map(|v| app_from(&v))
                    .unwrap_or_default())
            }
        }
    }

    async fn set_app(
        &self,
        a: &AppPolicy,
        by: &str,
        at: u64,
    ) -> Result<(), ordo_identity::StoreFault> {
        match &self.backing {
            Backing::Memory(m) => {
                m.lock().expect("policies").app = *a;
                Ok(())
            }
            Backing::Pg(pool) => pool
                .rows(&format!(
                    r#"INSERT INTO "$deploymentsetting" ("key", "value", "updated_by", "updated_at")
                       VALUES ({}, {}, {}, {at})
                       ON CONFLICT ("key") DO UPDATE SET "value" = EXCLUDED."value",
                         "updated_by" = EXCLUDED."updated_by", "updated_at" = EXCLUDED."updated_at""#,
                    sql_lit(APP_KEY),
                    sql_lit(&app_json(a).to_string()),
                    sql_lit(by),
                ))
                .await
                .map(|_| ())
                .map_err(|e| crate::auth::store_failure::pool_fault(e, "the app developer's session policy could not be written")),
        }
    }

    pub async fn tenant(&self, space: &str) -> Result<Values, ordo_identity::StoreFault> {
        match &self.backing {
            Backing::Memory(m) => Ok(m
                .lock()
                .expect("policies")
                .tenants
                .get(space)
                .copied()
                .unwrap_or_default()),
            Backing::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "policy" FROM "$sessionpolicy" WHERE "space" = {}"#,
                        sql_lit(space)
                    ))
                    .await
                    .map_err(|e| {
                        crate::auth::store_failure::pool_fault(
                            e,
                            "the tenant's session policy could not be read",
                        )
                    })?;
                Ok(rows
                    .first()
                    .and_then(|r| r.first().cloned().flatten())
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .map(|v| values_from(&v))
                    .unwrap_or_default())
            }
        }
    }

    async fn set_tenant(
        &self,
        space: &str,
        v: &Values,
        by: &str,
        at: u64,
    ) -> Result<(), ordo_identity::StoreFault> {
        match &self.backing {
            Backing::Memory(m) => {
                m.lock()
                    .expect("policies")
                    .tenants
                    .insert(space.to_string(), *v);
                Ok(())
            }
            Backing::Pg(pool) => pool
                .rows(&format!(
                    r#"INSERT INTO "$sessionpolicy" ("space", "policy", "updated_by", "updated_at")
                       VALUES ({}, {}, {}, {at})
                       ON CONFLICT ("space") DO UPDATE SET "policy" = EXCLUDED."policy",
                         "updated_by" = EXCLUDED."updated_by", "updated_at" = EXCLUDED."updated_at""#,
                    sql_lit(space),
                    sql_lit(&values_json(v).to_string()),
                    sql_lit(by),
                ))
                .await
                .map(|_| ())
                .map_err(|e| crate::auth::store_failure::pool_fault(e, "the tenant's session policy could not be written")),
        }
    }

    /// What is in force for a session for `space`. No space, or a store that
    /// cannot be read, is the deployment's and the app developer's layers
    /// alone, and then the deployment's alone: a session is never refused
    /// for a policy read.
    pub async fn effective(&self, space: Option<&str>) -> Effective {
        let app = self.app().await.unwrap_or_default();
        let tenant = match space {
            Some(s) => self.tenant(s).await.unwrap_or_default(),
            None => Values::default(),
        };
        resolve(&self.deployment, &app, &tenant)
    }

    /// Remember which tenant `session` is for, until `until`.
    pub async fn note_session(&self, session: &str, space: &str, until: u64) {
        match &self.backing {
            Backing::Memory(m) => {
                m.lock()
                    .expect("policies")
                    .sessions
                    .insert(session.to_string(), (space.to_string(), until));
            }
            Backing::Pg(pool) => {
                let _ = pool
                    .rows(&format!(
                        r#"INSERT INTO "$sessionspace" ("session_id", "space", "until")
                           VALUES ({}, {}, {until})
                           ON CONFLICT ("session_id") DO UPDATE SET "space" = EXCLUDED."space",
                             "until" = GREATEST("$sessionspace"."until", EXCLUDED."until")"#,
                        sql_lit(session),
                        sql_lit(space),
                    ))
                    .await;
            }
        }
    }

    /// The tenant `session` is for, when one was noted.
    pub async fn session_space(&self, session: &str) -> Option<String> {
        match &self.backing {
            Backing::Memory(m) => m
                .lock()
                .expect("policies")
                .sessions
                .get(session)
                .map(|(s, _)| s.clone()),
            Backing::Pg(pool) => pool
                .rows(&format!(
                    r#"SELECT "space" FROM "$sessionspace" WHERE "session_id" = {}"#,
                    sql_lit(session)
                ))
                .await
                .ok()?
                .first()
                .and_then(|r| r.first().cloned().flatten()),
        }
    }

    /// Drop sessions past their ceiling.
    pub async fn sweep(&self, now: u64) -> Result<(), String> {
        match &self.backing {
            Backing::Memory(m) => {
                m.lock()
                    .expect("policies")
                    .sessions
                    .retain(|_, (_, until)| *until > now);
                Ok(())
            }
            Backing::Pg(pool) => pool
                .rows(&format!(
                    r#"DELETE FROM "$sessionspace" WHERE "until" <= {now}"#
                ))
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    }

    /// The deployment and app developer layers, for `/health/deep`.
    pub async fn readback(&self) -> Value {
        let d = &self.deployment;
        let app = self.app().await.ok();
        let base = resolve(d, &app.unwrap_or_default(), &Values::default());
        json!({
            "deployment": {
                "tokenSecs": d.token_secs,
                "maxSecs": d.max_secs,
                "remember": if d.remember { "on" } else { "off" },
                "rememberSecs": d.remember_secs,
                "limits": {
                    "tokenSecs": format!("{TOKEN_FLOOR}-{}", d.token_limit),
                    "maxSecs": format!("{TOKEN_FLOOR}-{}", d.max_limit),
                    "rememberSecs": format!("{TOKEN_FLOOR}-{}", d.remember_limit),
                    "rememberMaxSecs": d.remember_max_limit
                        .map(|l| json!(format!("{TOKEN_FLOOR}-{l}")))
                        .unwrap_or(json!("none")),
                },
                "rememberMaxSecs": d.remember_max_secs.map_or(json!("none"), |v| json!(v)),
            },
            "app": app.map(|a| app_json(&a)),
            "inForce": effective_json(&base),
        })
    }

    /// Run one of this module's ops.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op {
            SET_APP => self.set_app_op(space, bound, caller).await,
            SET_TENANT => self.set_tenant_op(space, bound, caller).await,
            READ_TENANT => self.read_tenant(space).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a session policy op"
            ))),
        }
    }

    async fn set_app_op(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if self.home.as_deref().is_some_and(|h| h != space) {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "the app developer's session policy is the deployment's setting and this op \
                 answers only in the deployment's own space",
            ));
        }
        let mut a = self.app().await.map_err(unavailable)?;
        apply(&mut a.values.token_secs, secs_change(bound, "tokenSecs")?);
        apply(&mut a.values.max_secs, secs_change(bound, "maxSecs")?);
        apply(&mut a.values.remember, remember_change(bound)?);
        apply(
            &mut a.values.remember_secs,
            secs_change(bound, "rememberSecs")?,
        );
        apply(
            &mut a.ranges.token_secs,
            range_change(bound, "tokenSecsRange")?,
        );
        apply(&mut a.ranges.max_secs, range_change(bound, "maxSecsRange")?);
        apply(
            &mut a.ranges.remember_secs,
            range_change(bound, "rememberSecsRange")?,
        );
        apply(&mut a.values.remember_max_secs, overall_change(bound)?);
        apply(
            &mut a.ranges.remember_max_secs,
            range_change(bound, "rememberMaxSecsRange")?,
        );
        let d = &self.deployment;
        let dep_overall = d.remember_max_limit.map(|lim| {
            (
                (TOKEN_FLOOR, lim),
                format!("the deployment's limit ({REMEMBER_MAX_LIMIT_ENV})"),
            )
        });
        if let Some(v) = a.values.remember_max_secs {
            overall_check(v, dep_overall.clone())?;
        }
        if let Some((rlo, rhi)) = a.ranges.remember_max_secs {
            let (lo, hi) = dep_overall
                .as_ref()
                .map_or((TOKEN_FLOOR, u64::MAX), |(r, _)| *r);
            if rlo < lo || rhi > hi {
                return Err(refused(format!(
                    "rememberMaxSecsRange {rlo}-{rhi} is outside {lo}-{hi}, the deployment's \
                     limit ({REMEMBER_MAX_LIMIT_ENV})"
                )));
            }
        }
        for f in FIELDS {
            let (lo, hi) = d.limit(f);
            let why = format!("the deployment's limit ({})", f.limit_env());
            if let Some(v) = f.of(&a.values) {
                if v < lo || v > hi {
                    return Err(refused(format!(
                        "{} {v} is outside {lo}-{hi}, {why}",
                        f.param()
                    )));
                }
            }
            if let Some((rlo, rhi)) = f.range(&a.ranges) {
                if rlo < lo || rhi > hi {
                    return Err(refused(format!(
                        "{}Range {rlo}-{rhi} is outside {lo}-{hi}, {why}",
                        f.param()
                    )));
                }
            }
        }
        if a.values.remember == Some(true) && !d.remember {
            return Err(refused(format!(
                "remember on: this deployment keeps nobody signed in across a reload; the \
                 deployment owner sets {}=on to allow it",
                crate::renewal::SETTING
            )));
        }
        let now = (self.now)();
        if !matches!(bound.0.len(), 0) {
            self.set_app(&a, caller, now).await.map_err(unavailable)?;
            self.record(
                "session-policy-app-set",
                app_json(&a).to_string(),
                caller,
                space,
                now,
            );
        }
        let mut row = app_json(&a);
        let base = resolve(d, &a, &Values::default());
        row["inForce"] = effective_json(&base);
        Ok(ExecResult {
            position: None,
            rows: vec![row],
            next_cursor: None,
        })
    }

    async fn set_tenant_op(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let app = self.app().await.map_err(unavailable)?;
        let mut t = self.tenant(space).await.map_err(unavailable)?;
        apply(&mut t.token_secs, secs_change(bound, "tokenSecs")?);
        apply(&mut t.max_secs, secs_change(bound, "maxSecs")?);
        apply(&mut t.remember, remember_change(bound)?);
        apply(&mut t.remember_secs, secs_change(bound, "rememberSecs")?);
        apply(&mut t.remember_max_secs, overall_change(bound)?);
        if let Some(v) = t.remember_max_secs {
            overall_check(v, overall_range(&self.deployment, &app))?;
        }
        for f in FIELDS {
            if let Some(v) = f.of(&t) {
                let ((lo, hi), why) = tenant_range(&self.deployment, &app, f);
                if v < lo || v > hi {
                    return Err(refused(format!(
                        "{} {v} is outside {lo}-{hi}, {why}",
                        f.param()
                    )));
                }
            }
        }
        if t.remember == Some(true) && !self.deployment.remember {
            return Err(refused(format!(
                "remember on: this deployment keeps nobody signed in across a reload; the \
                 deployment owner sets {}=on to allow it",
                crate::renewal::SETTING
            )));
        }
        let now = (self.now)();
        self.set_tenant(space, &t, caller, now)
            .await
            .map_err(unavailable)?;
        self.record(
            "session-policy-tenant-set",
            values_json(&t).to_string(),
            caller,
            space,
            now,
        );
        self.read_tenant(space).await
    }

    async fn read_tenant(&self, space: &str) -> Result<ExecResult, ExecError> {
        let app = self.app().await.map_err(unavailable)?;
        let t = self.tenant(space).await.map_err(unavailable)?;
        let e = resolve(&self.deployment, &app, &t);
        let mut row = effective_json(&e);
        for f in FIELDS {
            let ((lo, hi), _) = tenant_range(&self.deployment, &app, f);
            row[format!("{}Range", f.param())] = json!(format!("{lo}-{hi}"));
        }
        row["rememberMaxSecsRange"] = overall_range(&self.deployment, &app)
            .map_or(json!("any, or none"), |((lo, hi), _)| {
                json!(format!("{lo}-{hi}"))
            });
        row["tenantChoice"] = values_json(&t);
        Ok(ExecResult {
            position: None,
            rows: vec![row],
            next_cursor: None,
        })
    }

    fn record(&self, act: &str, detail: String, by: &str, space: &str, at: u64) {
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act,
                    detail,
                    by: Some(by),
                    tenant: Some(space),
                    deployment: true,
                    at,
                },
            );
        }
    }
}

fn effective_json(e: &Effective) -> Value {
    json!({
        "tokenSecs": e.token_secs,
        "maxSecs": e.max_secs,
        "remember": if e.remember { "on" } else { "off" },
        "rememberSecs": e.remember_secs,
        "tokenSecsSource": e.sources[0].word(),
        "maxSecsSource": e.sources[1].word(),
        "rememberSource": e.sources[2].word(),
        "rememberSecsSource": e.sources[3].word(),
        "rememberMaxSecs": e.remember_max_secs.map_or(json!("none"), |v| json!(v)),
        "rememberMaxSecsSource": e.sources[4].word(),
    })
}

/// The output fields of the read op and the tenant setter.
pub const TENANT_FIELDS: [&str; 15] = [
    "tokenSecs",
    "maxSecs",
    "remember",
    "rememberSecs",
    "rememberMaxSecs",
    "tokenSecsSource",
    "maxSecsSource",
    "rememberSource",
    "rememberSecsSource",
    "rememberMaxSecsSource",
    "tokenSecsRange",
    "maxSecsRange",
    "rememberSecsRange",
    "rememberMaxSecsRange",
    "tenantChoice",
];

/// The output fields of the app developer's op.
pub const APP_FIELDS: [&str; 10] = [
    "tokenSecs",
    "maxSecs",
    "remember",
    "rememberSecs",
    "rememberMaxSecs",
    "tokenSecsRange",
    "maxSecsRange",
    "rememberSecsRange",
    "rememberMaxSecsRange",
    "inForce",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn d() -> Deployment {
        Deployment::with_defaults(3_600, 43_200, crate::renewal::RememberConfig::default())
    }

    #[test]
    fn a_tenant_shorter_than_the_app_default_takes_effect() {
        let app = AppPolicy {
            values: Values {
                token_secs: Some(1_800),
                ..Values::default()
            },
            ranges: Ranges::default(),
        };
        let t = Values {
            token_secs: Some(600),
            ..Values::default()
        };
        let e = resolve(&d(), &app, &t);
        assert_eq!(e.token_secs, 600);
        assert_eq!(e.sources[0], Source::Tenant);
    }

    #[test]
    fn no_tenant_value_falls_back_to_the_app_then_the_deployment() {
        let app = AppPolicy {
            values: Values {
                max_secs: Some(7_200),
                ..Values::default()
            },
            ranges: Ranges::default(),
        };
        let e = resolve(&d(), &app, &Values::default());
        assert_eq!((e.max_secs, e.sources[1]), (7_200, Source::App));
        assert_eq!((e.token_secs, e.sources[0]), (3_600, Source::Deployment));
    }

    #[test]
    fn a_stored_tenant_value_outside_a_later_range_is_passed_over() {
        let app = AppPolicy {
            values: Values::default(),
            ranges: Ranges {
                token_secs: Some((300, 900)),
                ..Ranges::default()
            },
        };
        let t = Values {
            token_secs: Some(3_000),
            ..Values::default()
        };
        let e = resolve(&d(), &app, &t);
        assert_eq!((e.token_secs, e.sources[0]), (3_600, Source::Deployment));
    }

    #[test]
    fn remember_off_at_the_deployment_is_absolute() {
        let off = Deployment::with_defaults(
            3_600,
            43_200,
            crate::renewal::RememberConfig {
                on: false,
                idle_secs: 600,
            },
        );
        let t = Values {
            remember: Some(true),
            ..Values::default()
        };
        assert!(!resolve(&off, &AppPolicy::default(), &t).remember);
    }

    #[test]
    fn a_token_never_outlives_its_session() {
        let t = Values {
            token_secs: Some(7_200),
            max_secs: Some(3_600),
            ..Values::default()
        };
        assert_eq!(resolve(&d(), &AppPolicy::default(), &t).token_secs, 3_600);
    }

    #[test]
    fn the_overall_limit_is_none_by_default() {
        let e = resolve(&d(), &AppPolicy::default(), &Values::default());
        assert_eq!(e.remember_max_secs, None);
        assert_eq!(e.ceiling_secs(), e.max_secs);
        assert_eq!(e.overall_end(1_000), None);
    }

    #[test]
    fn a_tenant_overall_limit_shorter_than_the_app_wins() {
        let app = AppPolicy {
            values: Values {
                remember_max_secs: Some(7 * 86_400),
                ..Values::default()
            },
            ranges: Ranges::default(),
        };
        let t = Values {
            remember_max_secs: Some(86_400),
            ..Values::default()
        };
        let e = resolve(&d(), &app, &t);
        assert_eq!(
            (e.remember_max_secs, e.sources[4]),
            (Some(86_400), Source::Tenant)
        );
        let e = resolve(&d(), &app, &Values::default());
        assert_eq!(
            (e.remember_max_secs, e.sources[4]),
            (Some(7 * 86_400), Source::App)
        );
        // A tenant may choose none when nothing above it caps the limit.
        let none = Values {
            remember_max_secs: Some(0),
            ..Values::default()
        };
        assert_eq!(resolve(&d(), &app, &none).remember_max_secs, None);
    }

    #[test]
    fn the_deployment_cap_is_absolute() {
        let capped = d().with_remember_max(None, Some(86_400));
        let e = resolve(&capped, &AppPolicy::default(), &Values::default());
        assert_eq!(
            e.remember_max_secs,
            Some(86_400),
            "nothing chosen ends at the cap"
        );
        let t = Values {
            remember_max_secs: Some(0),
            ..Values::default()
        };
        assert_eq!(
            resolve(&capped, &AppPolicy::default(), &t).remember_max_secs,
            Some(86_400),
            "none is outside the cap and passed over"
        );
        let env = |k: &str| match k {
            REMEMBER_MAX_ENV => Some("200000".to_string()),
            REMEMBER_MAX_LIMIT_ENV => Some("86400".to_string()),
            _ => None,
        };
        let e = Deployment::from_vars(
            &env,
            3_600,
            43_200,
            crate::renewal::RememberConfig::default(),
        )
        .unwrap_err();
        assert_eq!(e.0, REMEMBER_MAX_ENV);
        assert!(
            e.1.contains("ORDO_REMEMBER_MAX_SECS_LIMIT=86400"),
            "{}",
            e.1
        );
    }

    #[test]
    fn a_default_above_its_limit_refuses_the_boot_naming_both() {
        let env = |k: &str| (k == TOKEN_LIMIT_ENV).then(|| "600".to_string());
        let e = Deployment::from_vars(
            &env,
            3_600,
            43_200,
            crate::renewal::RememberConfig::default(),
        )
        .unwrap_err();
        assert_eq!(e.0, "ORDO_SESSION_SECS");
        assert!(e.1.contains("ORDO_SESSION_SECS_LIMIT=600"), "{}", e.1);
    }
}
