//! The public path's settings and its CDN purge hook
//! (`design-public-path.md` §3, §9; build steps 3 and 4).
//!
//! The cache itself is `ordo_api::public`. This module reads its settings at
//! boot and supplies the one piece `ordo-api` cannot: a POST to the
//! deployment's CDN through `ordo-egress`, so the destination passes the same
//! pins every other outbound call does.
//!
//! | Variable | Default | What it bounds |
//! |---|---|---|
//! | `ORDO_PUBLIC_CACHE_BYTES` | 268435456 (256 MiB) | the result cache per node; `0` turns it off |
//! | `ORDO_PUBLIC_CDN_S_MAXAGE` | 60 | seconds a shared cache may hold a public answer |
//! | `ORDO_PUBLIC_CDN_SWR` | 300 | seconds a shared cache may serve stale while it refetches |
//! | `ORDO_PUBLIC_MISSES_PER_MINUTE` | 600 | cache misses one public op may send to the database per minute, per node; `0` turns the cap off |
//! | `ORDO_PUBLIC_CDN_PURGE_URL` | unset | an `https` address the purge hook posts dropped tags to; unset posts nothing |
//! | `ORDO_PUBLIC_CDN_PURGE_SECRET_FILE` | unset | a file holding the bearer secret the hook sends |
//! | `ORDO_PUBLIC_CDN_PURGE_TIMEOUT_SECS` | 5 | how long one purge call may take, 1 to 60 |
//!
//! A purge is `POST <url>` with `{"tags": ["op:<name>", "entity:<name>", …]}`
//! and, when a secret is set, `Authorization: Bearer <secret>`. Every CDN
//! names its purge call differently, so the deployment owner points this at
//! the CDN's purge endpoint or at a small relay of their own.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use ordo_api::{PublicSettings, PurgeHook};
use ordo_egress::{Allowlist, DenyList, FetchMethod, FetchRequest, HttpsUrl, TlsRoots};
use ordo_identity::federation::ClientSecret;

pub const CACHE_BYTES: &str = "ORDO_PUBLIC_CACHE_BYTES";
pub const S_MAXAGE: &str = "ORDO_PUBLIC_CDN_S_MAXAGE";
pub const SWR: &str = "ORDO_PUBLIC_CDN_SWR";
pub const MISSES_PER_MINUTE: &str = "ORDO_PUBLIC_MISSES_PER_MINUTE";

/// The largest result cache a node may be given: 64 GiB.
pub const CACHE_BYTES_MAX: usize = 64 * 1024 * 1024 * 1024;
/// The longest a shared cache may be told to hold or serve stale: 7 days.
pub const CDN_SECS_MAX: u64 = 7 * 24 * 3600;
/// The highest per-op miss cap: one million a minute.
pub const MISSES_MAX: u32 = 1_000_000;
pub const PURGE_URL: &str = "ORDO_PUBLIC_CDN_PURGE_URL";
pub const PURGE_SECRET_FILE: &str = "ORDO_PUBLIC_CDN_PURGE_SECRET_FILE";
pub const PURGE_TIMEOUT_ENV: &str = "ORDO_PUBLIC_CDN_PURGE_TIMEOUT_SECS";

/// How long one purge call may take, connect to last byte, by default.
/// `ORDO_PUBLIC_CDN_PURGE_TIMEOUT_SECS` sets it, 1 to 60.
pub const DEFAULT_PURGE_TIMEOUT_SECS: u64 = 5;
pub const MAX_PURGE_TIMEOUT_SECS: u64 = 60;
/// The largest purge answer read. Nothing in it is used, so this bounds
/// only what a CDN can make the deployment read. Fixed on purpose.
const PURGE_CAP: usize = 16 * 1024;

/// Read the cache and CDN settings. An unset variable is its default; a
/// value that is not a whole number in its range is refused, naming the
/// variable, the range and the default.
pub fn settings_from_vars(
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<PublicSettings, (&'static str, String)> {
    let d = PublicSettings::default();
    fn number<T: std::str::FromStr + std::fmt::Display + Copy + PartialOrd>(
        var: &dyn Fn(&str) -> Option<String>,
        name: &'static str,
        default: T,
        lo: T,
        hi: T,
    ) -> Result<T, (&'static str, String)> {
        match var(name).map(|v| v.trim().to_string()) {
            None => Ok(default),
            Some(v) if v.is_empty() => Ok(default),
            Some(v) => match v.parse::<T>() {
                Ok(n) if n >= lo && n <= hi => Ok(n),
                _ => Err((
                    name,
                    format!(
                        "`{v}` is not a whole number from {lo} to {hi}; set one in that \
                         range, or unset it for the default, {default}"
                    ),
                )),
            },
        }
    }
    let human_check = match var(HUMAN_CHECK).map(|v| v.trim().to_ascii_lowercase()) {
        None => d.human_check,
        Some(v) if v.is_empty() || v == "on" => true,
        Some(v) if v == "off" => false,
        Some(v) => {
            return Err((
                HUMAN_CHECK,
                format!("`{v}` is not `on` or `off`; unset it for the default, on"),
            ))
        }
    };
    Ok(PublicSettings {
        cache_bytes: number(var, CACHE_BYTES, d.cache_bytes, 0, CACHE_BYTES_MAX)?,
        s_maxage: number(var, S_MAXAGE, d.s_maxage, 0, CDN_SECS_MAX)?,
        swr: number(var, SWR, d.swr, 0, CDN_SECS_MAX)?,
        misses_per_minute: number(var, MISSES_PER_MINUTE, d.misses_per_minute, 0, MISSES_MAX)?,
        writes_per_hour: number(var, WRITES_PER_HOUR, d.writes_per_hour, 0, MISSES_MAX)?,
        human_check,
    })
}

pub const WRITES_PER_HOUR: &str = "ORDO_PUBLIC_WRITES_PER_HOUR";
pub const HUMAN_CHECK: &str = "ORDO_PUBLIC_HUMAN_CHECK";
pub const HUMAN_PROVIDER: &str = "ORDO_PUBLIC_HUMAN_PROVIDER";
pub const HUMAN_SECRET_FILE: &str = "ORDO_PUBLIC_HUMAN_SECRET_FILE";
pub const HUMAN_VERIFY_SECS: &str = "ORDO_PUBLIC_HUMAN_VERIFY_SECS";
pub const HUMAN_VERIFY_URL: &str = "ORDO_PUBLIC_HUMAN_VERIFY_URL";
/// The sign-in check's settings the public check falls back to.
pub const AUTH_SECRET_FILE: &str = "ORDO_AUTH_HUMAN_CHECK_SECRET_FILE";
pub const AUTH_VERIFY_SECS: &str = "ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS";
pub const AUTH_VERIFY_URL: &str = "ORDO_AUTH_HUMAN_CHECK_VERIFY_URL";
/// The longest one verification may take, as the sign-in check's.
pub const HUMAN_VERIFY_SECS_MAX: u64 = 30;
/// The largest answer read from a provider. Fixed on purpose: the answer is
/// a small JSON object, and this bounds what a provider can make us read.
const HUMAN_CAP: usize = 16 * 1024;

/// The human-check providers Ordo verifies with (ruled 2026-10-03,
/// `public-path-design` ask 3): each is one form POST of the secret and the
/// widget's token, answered `{"success": true|false, …}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HumanProvider {
    Turnstile,
    Hcaptcha,
    Recaptcha,
}

impl HumanProvider {
    pub fn parse(v: &str) -> Option<HumanProvider> {
        match v.trim().to_ascii_lowercase().as_str() {
            "" | "turnstile" => Some(HumanProvider::Turnstile),
            "hcaptcha" => Some(HumanProvider::Hcaptcha),
            "recaptcha" => Some(HumanProvider::Recaptcha),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            HumanProvider::Turnstile => "turnstile",
            HumanProvider::Hcaptcha => "hcaptcha",
            HumanProvider::Recaptcha => "recaptcha",
        }
    }

    /// Each provider's documented verify address.
    pub fn verify_url(self) -> &'static str {
        match self {
            HumanProvider::Turnstile => "https://challenges.cloudflare.com/turnstile/v0/siteverify",
            HumanProvider::Hcaptcha => "https://api.hcaptcha.com/siteverify",
            HumanProvider::Recaptcha => "https://www.google.com/recaptcha/api/siteverify",
        }
    }
}

/// Read a provider's answer: `success: true` passes, anything else fails.
pub fn human_verdict(body: &[u8]) -> ordo_api::HumanVerdict {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(v) if v.get("success") == Some(&serde_json::Value::Bool(true)) => {
            ordo_api::HumanVerdict::Pass
        }
        Ok(_) => ordo_api::HumanVerdict::Fail,
        Err(_) => ordo_api::HumanVerdict::Unavailable("the provider's answer is not JSON".into()),
    }
}

/// The human check: one form POST per write, through `ordo-egress`. Holds
/// the secret, so it has no `Debug`. The visitor's address is not sent: the
/// providers accept it as optional, and Ordo does not hand addresses out.
pub struct EgressHuman {
    provider: HumanProvider,
    url: HttpsUrl,
    allow: Allowlist,
    deny: DenyList,
    roots: TlsRoots,
    secret: ClientSecret,
    timeout: Duration,
}

impl ordo_api::HumanVerifier for EgressHuman {
    fn provider(&self) -> &'static str {
        self.provider.name()
    }

    fn verify(&self, token: String) -> BoxFuture<'static, ordo_api::HumanVerdict> {
        let url = self.url.clone();
        let allow = self.allow.clone();
        let deny = self.deny.clone();
        let roots = self.roots.clone();
        let timeout = self.timeout;
        let body = ordo_identity::federation::form_encode(&[
            ("secret", self.secret.reveal_to_wire()),
            ("response", &token),
        ]);
        Box::pin(async move {
            let dest = match ordo_egress::resolve_and_pin(&url, &allow, &deny).await {
                Ok(d) => d,
                Err(e) => return ordo_api::HumanVerdict::Unavailable(e.disclosable()),
            };
            match ordo_egress::fetch(
                dest,
                &roots,
                FetchRequest {
                    method: FetchMethod::PostForm,
                    body: body.as_bytes(),
                    accept: "application/json",
                    headers: &[],
                },
                HUMAN_CAP,
                timeout,
            )
            .await
            {
                Ok(f) => human_verdict(&f.into_verifier_input()),
                Err(e) => ordo_api::HumanVerdict::Unavailable(match e {
                    ordo_egress::Refusal::TimedOut { .. } => format!(
                        "{}; a slow provider can be given longer with {HUMAN_VERIFY_SECS}",
                        e.disclosable()
                    ),
                    _ => e.disclosable(),
                }),
            }
        })
    }
}

/// Read the human check's provider. `Ok(None)` when neither secret file is set:
/// with `ORDO_PUBLIC_HUMAN_CHECK` on, public writes are then refused, and
/// the refusal names both settings.
pub fn human_from_vars(
    var: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&str) -> Result<String, String>,
    deny: DenyList,
) -> Result<Option<Arc<dyn ordo_api::HumanVerifier>>, (&'static str, String)> {
    if var("ORDO_PUBLIC_HUMAN_SECRET").is_some() {
        return Err((
            "ORDO_PUBLIC_HUMAN_SECRET",
            format!(
                "not read: a secret in a variable's value shows in a process listing; put it \
                 in a file and name the file in {HUMAN_SECRET_FILE}"
            ),
        ));
    }
    let raw = var(HUMAN_PROVIDER).unwrap_or_default();
    let provider = HumanProvider::parse(&raw).ok_or_else(|| {
        (
            HUMAN_PROVIDER,
            format!(
                "`{}` is not a provider Ordo checks with; set turnstile, hcaptcha or \
                 recaptcha, or unset it for the default, turnstile",
                raw.trim()
            ),
        )
    })?;
    // Layered on the sign-in check: each public value falls back to its
    // `ORDO_AUTH_HUMAN_CHECK_*` counterpart, and overrides it when set, so a
    // deployment configures Turnstile once. The sign-in check is Turnstile
    // only, so its secret and address serve a Turnstile public check only.
    let get = |k: &str| {
        var(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let turnstile = provider == HumanProvider::Turnstile;
    let layered = |own: &'static str, auth: &'static str| -> Option<(&'static str, String)> {
        get(own)
            .map(|v| (own, v))
            .or_else(|| turnstile.then(|| get(auth).map(|v| (auth, v))).flatten())
    };
    let Some((file_var, path)) = layered(HUMAN_SECRET_FILE, AUTH_SECRET_FILE) else {
        if !turnstile && get(AUTH_SECRET_FILE).is_some() {
            return Err((
                HUMAN_SECRET_FILE,
                format!(
                    "is required for {}: the sign-in secret in {AUTH_SECRET_FILE} is a \
                     Turnstile key and serves a Turnstile check only",
                    provider.name()
                ),
            ));
        }
        return Ok(None);
    };
    let secret = read_file(&path).map_err(|_| (file_var, "could not be read".to_string()))?;
    let secret = ClientSecret::new(secret.trim().to_string());
    if secret.is_empty() {
        return Err((file_var, "the secret file is empty".to_string()));
    }
    let timeout = match layered(HUMAN_VERIFY_SECS, AUTH_VERIFY_SECS) {
        None => DEFAULT_PURGE_TIMEOUT_SECS,
        Some((name, v)) => v
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=HUMAN_VERIFY_SECS_MAX).contains(n))
            .ok_or_else(|| {
                (
                    name,
                    format!(
                        "`{v}` is not a whole number from 1 to {HUMAN_VERIFY_SECS_MAX}; unset \
                         it for the default, {DEFAULT_PURGE_TIMEOUT_SECS}"
                    ),
                )
            })?,
    };
    let (url_var, url) = layered(HUMAN_VERIFY_URL, AUTH_VERIFY_URL)
        .unwrap_or((HUMAN_PROVIDER, provider.verify_url().to_string()));
    let url = HttpsUrl::parse(&url).map_err(|e| (url_var, e.disclosable()))?;
    let allow = Allowlist::exactly(url.host());
    Ok(Some(Arc::new(EgressHuman {
        provider,
        url,
        allow,
        deny,
        roots: TlsRoots::webpki(),
        secret,
        timeout: Duration::from_secs(timeout),
    })))
}

/// Public requests in the deployment's monitoring:
/// `ordo_public_requests_total{op, kind, outcome}`. `kind` is `read`,
/// `write` or `door`; `outcome` is `hit`, `miss`, `written`, `limited`,
/// `human_check_failed`, `failed` or `refused`.
pub struct MetricsMonitor(pub Arc<crate::metrics::Metrics>);

impl ordo_api::PublicMonitor for MetricsMonitor {
    fn count(&self, op: &str, kind: &'static str, outcome: &'static str) {
        self.0.incr(
            "ordo_public_requests_total",
            &[("op", op), ("kind", kind), ("outcome", outcome)],
        );
    }
}

/// The purge hook's log line for an egress refusal. A destination on a
/// private address names the setting that would allow it, as mail and
/// sign-in do (audit 2026-10-11, finding 23).
pub fn purge_refusal_text(r: &ordo_egress::Refusal) -> String {
    use ordo_egress::DeniedCategory as C;
    match r {
        ordo_egress::Refusal::DeniedAddress {
            host,
            category: C::Loopback | C::Private | C::UniqueLocal | C::Cgnat | C::DeploymentRange,
            ..
        } => format!(
            "{}; if the CDN purge endpoint is on the deployment's own network, add it to \
             {env} ({env}={host})",
            r.disclosable(),
            env = ordo_egress::PRIVATE_HOSTS_ENV,
        ),
        ordo_egress::Refusal::TimedOut { .. } => format!(
            "{}; a slow CDN purge endpoint can be given longer with {PURGE_TIMEOUT_ENV} \
             (up to {MAX_PURGE_TIMEOUT_SECS})",
            r.disclosable()
        ),
        _ => r.disclosable(),
    }
}

/// What a node does to the public path when a new version is installed: the
/// bus learns the new version's ops and entities, and the cache drops
/// everything. A deploy may move rows, and one rule is easier to trust than a
/// per-op diff (`design-public-path.md` §3, "Deploy"; ruled ask 11).
pub fn after_deploy(
    bus: &crate::invalidation::PgInvalidationBus,
    public: &ordo_api::PublicPath,
    fixture: &ordo_conformance::Fixture,
) {
    bus.reindex(fixture);
    public.drop_all();
}

/// The purge hook: one POST per drop, through `ordo-egress`.
/// Holds the secret, so it has no `Debug`.
pub struct CdnPurge {
    url: HttpsUrl,
    allow: Allowlist,
    deny: DenyList,
    roots: TlsRoots,
    secret: Option<ClientSecret>,
    timeout: Duration,
}

/// The body a purge posts.
pub fn purge_body(tags: &[String]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "tags": tags })).unwrap_or_default()
}

impl PurgeHook for CdnPurge {
    fn purge(&self, tags: Vec<String>) -> BoxFuture<'static, ()> {
        let url = self.url.clone();
        let allow = self.allow.clone();
        let deny = self.deny.clone();
        let roots = self.roots.clone();
        let timeout = self.timeout;
        let auth = self
            .secret
            .as_ref()
            .map(|s| format!("Bearer {}", s.reveal_to_wire()));
        Box::pin(async move {
            let body = purge_body(&tags);
            let dest = match ordo_egress::resolve_and_pin(&url, &allow, &deny).await {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("public cache: purge not sent ({})", purge_refusal_text(&e));
                    return;
                }
            };
            let headers: Vec<(&str, String)> =
                auth.into_iter().map(|a| ("authorization", a)).collect();
            if let Err(e) = ordo_egress::fetch(
                dest,
                &roots,
                FetchRequest {
                    method: FetchMethod::PostJson,
                    body: &body,
                    accept: "application/json",
                    headers: &headers,
                },
                PURGE_CAP,
                timeout,
            )
            .await
            {
                eprintln!(
                    "public cache: purge of {} tag(s) refused ({}); the CDN holds them for at \
                     most {S_MAXAGE}",
                    tags.len(),
                    purge_refusal_text(&e)
                );
            }
        })
    }
}

/// Read the purge hook. `Ok(None)` when `ORDO_PUBLIC_CDN_PURGE_URL` is unset:
/// no CDN, and nothing is posted.
pub fn purge_from_vars(
    var: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&str) -> Result<String, String>,
    deny: DenyList,
) -> Result<Option<Arc<dyn PurgeHook>>, (&'static str, String)> {
    let Some(raw) = var(PURGE_URL)
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
    else {
        return Ok(None);
    };
    if var("ORDO_PUBLIC_CDN_PURGE_SECRET").is_some() {
        return Err((
            "ORDO_PUBLIC_CDN_PURGE_SECRET",
            format!(
                "not read: a secret in a variable's value shows in a process listing; put it \
                 in a file and name the file in {PURGE_SECRET_FILE}"
            ),
        ));
    }
    let url = HttpsUrl::parse(&raw).map_err(|e| (PURGE_URL, e.disclosable()))?;
    let secret = match var(PURGE_SECRET_FILE)
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
    {
        None => None,
        Some(path) => {
            let s = read_file(&path)
                .map_err(|_| (PURGE_SECRET_FILE, "could not be read".to_string()))?;
            let s = ClientSecret::new(s.trim().to_string());
            if s.is_empty() {
                return Err((PURGE_SECRET_FILE, "the secret file is empty".to_string()));
            }
            Some(s)
        }
    };
    let timeout = match var(PURGE_TIMEOUT_ENV)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        None => DEFAULT_PURGE_TIMEOUT_SECS,
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=MAX_PURGE_TIMEOUT_SECS).contains(n))
            .ok_or_else(|| {
                (
                    PURGE_TIMEOUT_ENV,
                    format!(
                        "`{v}` is not a whole number from 1 to {MAX_PURGE_TIMEOUT_SECS}; unset \
                         it for the default, {DEFAULT_PURGE_TIMEOUT_SECS}"
                    ),
                )
            })?,
    };
    let allow = Allowlist::exactly(url.host());
    Ok(Some(Arc::new(CdnPurge {
        url,
        allow,
        deny,
        roots: TlsRoots::webpki(),
        secret,
        timeout: Duration::from_secs(timeout),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let m: Vec<_> = pairs.to_vec();
        move |k: &str| m.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn unset_settings_are_the_designs_defaults() {
        let s = settings_from_vars(&vars(&[])).unwrap();
        assert_eq!(s, PublicSettings::default());
        assert_eq!(s.s_maxage, 60);
        assert_eq!(s.swr, 300);
        assert_eq!(s.cache_bytes, 256 * 1024 * 1024);
    }

    #[test]
    fn settings_are_read_and_a_bad_one_names_its_variable() {
        let s = settings_from_vars(&vars(&[(S_MAXAGE, "5"), (SWR, "0"), (CACHE_BYTES, "1024")]))
            .unwrap();
        assert_eq!((s.s_maxage, s.swr, s.cache_bytes), (5, 0, 1024));
        let (var, detail) = settings_from_vars(&vars(&[(S_MAXAGE, "soon")])).unwrap_err();
        assert_eq!(var, S_MAXAGE);
        assert!(detail.contains("default, 60"), "{detail}");
    }

    #[test]
    fn a_number_past_its_bound_is_refused_with_range_and_default() {
        for (name, value, default) in [
            (S_MAXAGE, "999999999", "60"),
            (SWR, "604801", "300"),
            (CACHE_BYTES, "68719476737", "268435456"),
            (MISSES_PER_MINUTE, "1000001", "600"),
            (MISSES_PER_MINUTE, "-1", "600"),
        ] {
            let (var, detail) = settings_from_vars(&vars(&[(name, value)])).unwrap_err();
            assert_eq!(var, name);
            assert!(detail.contains(" from 0 to "), "{detail}");
            assert!(detail.contains(&format!("default, {default}")), "{detail}");
        }
        let s =
            settings_from_vars(&vars(&[(S_MAXAGE, "604800"), (MISSES_PER_MINUTE, "0")])).unwrap();
        assert_eq!((s.s_maxage, s.misses_per_minute), (604800, 0));
    }

    #[test]
    fn a_purge_to_a_private_address_names_the_private_hosts_list() {
        let r = ordo_egress::Refusal::DeniedAddress {
            host: "purge.internal".into(),
            address: "10.0.0.5".parse().unwrap(),
            category: ordo_egress::DeniedCategory::Private,
        };
        let text = purge_refusal_text(&r);
        assert!(
            text.contains("ORDO_EGRESS_PRIVATE_HOSTS=purge.internal"),
            "{text}"
        );
        assert!(!text.contains("10.0.0.5"), "the address stays out: {text}");
    }

    #[test]
    fn the_purge_timeout_is_a_bounded_setting() {
        let read = |_: &str| Ok("s".to_string());
        let url = (PURGE_URL, "https://cdn.example.com/purge");
        for ok in ["1", "60"] {
            assert!(purge_from_vars(
                &vars(&[url, (PURGE_TIMEOUT_ENV, ok)]),
                &read,
                DenyList::categories_only()
            )
            .is_ok());
        }
        for bad in ["0", "61", "soon"] {
            let (var, detail) = purge_from_vars(
                &vars(&[url, (PURGE_TIMEOUT_ENV, bad)]),
                &read,
                DenyList::categories_only(),
            )
            .err()
            .expect("refused");
            assert_eq!(var, PURGE_TIMEOUT_ENV);
            assert!(
                detail.contains("from 1 to 60") && detail.contains("default, 5"),
                "{detail}"
            );
        }
        let t = purge_refusal_text(&ordo_egress::Refusal::TimedOut { after_ms: 5000 });
        assert!(t.contains(PURGE_TIMEOUT_ENV), "{t}");
    }

    #[test]
    fn an_unset_hook_posts_nothing() {
        let read = |_: &str| Ok("s".to_string());
        let hook = purge_from_vars(&vars(&[]), &read, DenyList::categories_only()).expect("reads");
        assert!(hook.is_none());
    }

    #[test]
    fn the_hook_wants_https_and_a_secret_in_a_file() {
        let read = |_: &str| Ok("s3cret".to_string());
        let deny = DenyList::categories_only;
        assert!(purge_from_vars(
            &vars(&[(PURGE_URL, "https://cdn.example.com/purge")]),
            &read,
            deny()
        )
        .unwrap()
        .is_some());
        let (var, _) = purge_from_vars(
            &vars(&[(PURGE_URL, "http://cdn.example.com/purge")]),
            &read,
            deny(),
        )
        .err()
        .expect("plain http is refused");
        assert_eq!(var, PURGE_URL);
        let (var, _) = purge_from_vars(
            &vars(&[
                (PURGE_URL, "https://cdn.example.com/purge"),
                ("ORDO_PUBLIC_CDN_PURGE_SECRET", "s3cret"),
            ]),
            &read,
            deny(),
        )
        .err()
        .expect("a secret in a variable is refused");
        assert_eq!(var, "ORDO_PUBLIC_CDN_PURGE_SECRET");
    }

    #[test]
    fn public_write_settings_default_to_a_cap_and_the_check_on() {
        let s = settings_from_vars(&vars(&[])).unwrap();
        assert_eq!((s.writes_per_hour, s.human_check), (10_000, true));
        let s = settings_from_vars(&vars(&[(WRITES_PER_HOUR, "0"), (HUMAN_CHECK, "OFF")])).unwrap();
        assert_eq!((s.writes_per_hour, s.human_check), (0, false));
        let (var, detail) = settings_from_vars(&vars(&[(HUMAN_CHECK, "maybe")])).unwrap_err();
        assert_eq!(var, HUMAN_CHECK);
        assert!(detail.contains("`on` or `off`"), "{detail}");
        let (var, _) = settings_from_vars(&vars(&[(WRITES_PER_HOUR, "lots")])).unwrap_err();
        assert_eq!(var, WRITES_PER_HOUR);
    }

    #[test]
    fn the_public_check_falls_back_to_the_sign_in_one_and_overrides_it() {
        let deny = DenyList::categories_only;
        let read_from = std::cell::RefCell::new(Vec::<String>::new());
        let read = |p: &str| {
            read_from.borrow_mut().push(p.to_string());
            Ok("s3cret".to_string())
        };
        // The sign-in secret alone serves a Turnstile public check.
        let h = human_from_vars(&vars(&[(AUTH_SECRET_FILE, "/auth")]), &read, deny())
            .unwrap()
            .expect("the sign-in secret serves");
        assert_eq!(h.provider(), "turnstile");
        assert_eq!(read_from.borrow().last().unwrap(), "/auth");
        // A public secret overrides it.
        human_from_vars(
            &vars(&[(AUTH_SECRET_FILE, "/auth"), (HUMAN_SECRET_FILE, "/public")]),
            &read,
            deny(),
        )
        .unwrap()
        .expect("a provider");
        assert_eq!(read_from.borrow().last().unwrap(), "/public");
        // The sign-in address and timeout are read, and named when wrong.
        let (var, _) = human_from_vars(
            &vars(&[
                (AUTH_SECRET_FILE, "/auth"),
                (AUTH_VERIFY_URL, "http://plain.example"),
            ]),
            &read,
            deny(),
        )
        .err()
        .expect("plain http is refused");
        assert_eq!(var, AUTH_VERIFY_URL);
        let (var, _) = human_from_vars(
            &vars(&[(AUTH_SECRET_FILE, "/auth"), (AUTH_VERIFY_SECS, "99")]),
            &read,
            deny(),
        )
        .err()
        .expect("past the range");
        assert_eq!(var, AUTH_VERIFY_SECS);
        // The public value overrides the sign-in one.
        assert!(human_from_vars(
            &vars(&[
                (AUTH_SECRET_FILE, "/auth"),
                (AUTH_VERIFY_SECS, "99"),
                (HUMAN_VERIFY_SECS, "10"),
            ]),
            &read,
            deny(),
        )
        .is_ok());
        // A Turnstile sign-in secret never serves another provider.
        let (var, detail) = human_from_vars(
            &vars(&[(AUTH_SECRET_FILE, "/auth"), (HUMAN_PROVIDER, "hcaptcha")]),
            &read,
            deny(),
        )
        .err()
        .expect("refused");
        assert_eq!(var, HUMAN_SECRET_FILE);
        assert!(detail.contains(AUTH_SECRET_FILE), "{detail}");
    }

    #[test]
    fn the_human_check_reads_its_provider_and_a_secret_in_a_file() {
        let read = |_: &str| Ok("s3cret".to_string());
        let deny = DenyList::categories_only;
        assert!(human_from_vars(&vars(&[]), &read, deny())
            .unwrap()
            .is_none());
        for (name, provider) in [
            ("", "turnstile"),
            ("hcaptcha", "hcaptcha"),
            ("reCAPTCHA", "recaptcha"),
        ] {
            let v: &'static str = Box::leak(name.to_string().into_boxed_str());
            let h = human_from_vars(
                &vars(&[(HUMAN_PROVIDER, v), (HUMAN_SECRET_FILE, "/secret")]),
                &read,
                deny(),
            )
            .unwrap()
            .expect("a provider");
            assert_eq!(h.provider(), provider);
        }
        let (var, detail) = human_from_vars(
            &vars(&[
                (HUMAN_PROVIDER, "captchaco"),
                (HUMAN_SECRET_FILE, "/secret"),
            ]),
            &read,
            deny(),
        )
        .err()
        .expect("an unknown provider is refused");
        assert_eq!(var, HUMAN_PROVIDER);
        assert!(
            detail.contains("turnstile, hcaptcha or recaptcha"),
            "{detail}"
        );
        let (var, _) = human_from_vars(&vars(&[("ORDO_PUBLIC_HUMAN_SECRET", "x")]), &read, deny())
            .err()
            .expect("a secret in a variable is refused");
        assert_eq!(var, "ORDO_PUBLIC_HUMAN_SECRET");
        let empty = |_: &str| Ok("  ".to_string());
        let (var, _) = human_from_vars(&vars(&[(HUMAN_SECRET_FILE, "/secret")]), &empty, deny())
            .err()
            .expect("an empty secret is refused");
        assert_eq!(var, HUMAN_SECRET_FILE);
    }

    #[test]
    fn a_provider_answer_passes_only_on_success_true() {
        assert_eq!(
            human_verdict(br#"{"success":true}"#),
            ordo_api::HumanVerdict::Pass
        );
        assert_eq!(
            human_verdict(br#"{"success":false,"error-codes":["invalid-input-response"]}"#),
            ordo_api::HumanVerdict::Fail
        );
        assert_eq!(
            human_verdict(br#"{"success":"true"}"#),
            ordo_api::HumanVerdict::Fail
        );
        assert!(matches!(
            human_verdict(b"<html>"),
            ordo_api::HumanVerdict::Unavailable(_)
        ));
    }

    #[test]
    fn the_body_names_the_tags() {
        let body = purge_body(&["op:pricing".into(), "entity:plans".into()]);
        assert_eq!(
            String::from_utf8(body).unwrap(),
            r#"{"tags":["op:pricing","entity:plans"]}"#
        );
    }
}
