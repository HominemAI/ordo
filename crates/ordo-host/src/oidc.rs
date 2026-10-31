//! OIDC sign-in, the deployment's half (`plan-auth-federated.md` §4, §5.3):
//! a connection from its settings, and the transports a connection reaches
//! its issuer through.
//!
//! The provider itself is `ordo_identity::oidc`. What lives here is what
//! needs the environment and the network: reading `ORDO_AUTH_<NAME>_*`, the
//! secret file, and [`EgressFactory`], which gives discovery a transport whose
//! allowlist is the issuer's host and gives the endpoints discovery named a
//! transport whose allowlist is exactly theirs. Every request goes through the
//! egress kernel: https only, certificates validated, no redirects, the
//! deployment's deny list, and `ORDO_EGRESS_PRIVATE_HOSTS` for an issuer
//! inside the deployment's own network.

use std::sync::Arc;

use ordo_egress::{DenyList, HttpsUrl, TlsRoots};
use ordo_identity::federation::{ClientSecret, FederationTransport};
use ordo_identity::jose::{parse_alg_list, Alg, ALL_ALGS};
use ordo_identity::oauth2::valid_connection_name;
use ordo_identity::oidc::{
    microsoft_issuer, oidc_preset, OidcConnection, OidcEndpoints, TransportFactory,
    DEFAULT_CACHE_SECS, DEFAULT_SKEW_SECS, JWKS_STALE_GRACE_SECS, MAX_CACHE_SECS, MAX_SKEW_SECS,
    MAX_STALE_GRACE_SECS, MIN_CACHE_SECS, OIDC_PRESETS, PLAIN_DISPLAY_CLAIMS,
};

/// How long every OIDC connection keeps an issuer's keys and discovery
/// document. A provider's `max-age` shortens it.
pub const CACHE_ENV: &str = "ORDO_AUTH_OIDC_CACHE_SECS";
/// The clock skew every OIDC connection allows unless its own
/// `_CLOCK_SKEW_SECS` says otherwise. A tenant's connection always uses it.
pub const SKEW_ENV: &str = "ORDO_AUTH_OIDC_CLOCK_SKEW_SECS";
/// The ID token algorithms every OIDC connection accepts unless its own
/// `_ALGS` says otherwise. A tenant's connection always uses it.
pub const ALGS_ENV: &str = "ORDO_AUTH_OIDC_ALGS";
/// How long past freshness cached keys still verify while an issuer cannot
/// be reached, for every OIDC connection.
pub const STALE_GRACE_ENV: &str = "ORDO_AUTH_OIDC_STALE_GRACE_SECS";

/// The deployment's OIDC settings: the layer under every connection. A
/// deployment connection may override skew and algorithms with its own
/// `_CLOCK_SKEW_SECS` and `_ALGS`. A tenant's connection carries no value of
/// its own for them, so it takes these (audit 2026-10-11, finding 18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcDefaults {
    pub cache_secs: u64,
    pub skew_secs: u64,
    pub algs: Vec<Alg>,
    pub stale_grace_secs: u64,
}

impl Default for OidcDefaults {
    fn default() -> OidcDefaults {
        OidcDefaults {
            cache_secs: DEFAULT_CACHE_SECS,
            skew_secs: DEFAULT_SKEW_SECS,
            algs: ALL_ALGS.to_vec(),
            stale_grace_secs: JWKS_STALE_GRACE_SECS,
        }
    }
}

fn ranged(
    var: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
    default: u64,
    lo: u64,
    hi: u64,
) -> Result<u64, (&'static str, String)> {
    match var(name).map(|v| v.trim().to_string()) {
        None => Ok(default),
        Some(v) if v.is_empty() => Ok(default),
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|s| (lo..=hi).contains(s))
            .ok_or_else(|| {
                (
                    name,
                    format!(
                        "`{v}` is not a whole number from {lo} to {hi}; unset it for the \
                         default, {default}"
                    ),
                )
            }),
    }
}

/// Read [`OidcDefaults`]. A bad value names the setting, its range and its
/// default; the error's text leaves the name to the caller.
pub fn defaults_from_vars(
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<OidcDefaults, (&'static str, String)> {
    let cache_secs = cache_secs_from_vars(var).map_err(|d| (CACHE_ENV, d))?;
    let skew_secs = ranged(var, SKEW_ENV, DEFAULT_SKEW_SECS, 0, MAX_SKEW_SECS)?;
    let stale_grace_secs = ranged(
        var,
        STALE_GRACE_ENV,
        JWKS_STALE_GRACE_SECS,
        0,
        MAX_STALE_GRACE_SECS,
    )?;
    let algs = match var(ALGS_ENV)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        None => ALL_ALGS.to_vec(),
        Some(list) => parse_alg_list(&list).map_err(|e| {
            (
                ALGS_ENV,
                format!(
                    "a list of RS256, PS256, ES256 and EdDSA ({}); unset it for all four. \
                     `none` and HS* are never accepted",
                    e.0
                ),
            )
        })?,
    };
    Ok(OidcDefaults {
        cache_secs,
        skew_secs,
        algs,
        stale_grace_secs,
    })
}

use crate::federation::{prefix, redirect_uri, EgressTransport};

/// Is `name` an OIDC connection? It is when its preset is an OIDC preset or
/// its kind is `oidc`.
pub fn is_oidc(name: &str, var: &dyn Fn(&str) -> Option<String>) -> bool {
    let p = prefix(name);
    let word = |k: &str| var(&format!("{p}{k}")).map(|v| v.trim().to_string());
    word("PRESET").is_some_and(|w| OIDC_PRESETS.contains(&w.as_str()))
        || word("KIND").as_deref() == Some("oidc")
}

/// Build one OIDC connection from its settings.
///
/// | Setting | |
/// |---|---|
/// | `ORDO_AUTH_<NAME>_PRESET` | `google` or `microsoft` |
/// | `ORDO_AUTH_<NAME>_KIND` | `oidc`, for any provider with discovery |
/// | `ORDO_AUTH_<NAME>_ISSUER` | required without a preset; Google's is filled |
/// | `ORDO_AUTH_<NAME>_TENANT` | `microsoft`: the directory (tenant) id, a GUID |
/// | `ORDO_AUTH_<NAME>_CLIENT_ID`, `_CLIENT_SECRET_FILE` | required |
/// | `_SCOPE`, `_DISPLAY_CLAIMS`, `_AVATAR_CLAIM`, `_USE_EMAIL` | override the preset |
/// | `_AUTHORIZE_URL`, `_TOKEN_URL`, `_JWKS_URL` | all three pin the endpoints and skip discovery |
/// | `_ISS_PARAMETER` | `required`: with pinned endpoints, demand RFC 9207's `iss` |
/// | `_CLOCK_SKEW_SECS` | 0 to 120, default `ORDO_AUTH_OIDC_CLOCK_SKEW_SECS` (60) |
/// | `_ALGS` | a subset of `RS256,PS256,ES256,EdDSA`, default `ORDO_AUTH_OIDC_ALGS` (all four) |
/// | `ORDO_AUTH_OIDC_STALE_GRACE_SECS` | deployment-wide: old keys verify this long while the issuer is down, 0 to 604800, default 86400 |
/// | `ORDO_AUTH_OIDC_CACHE_SECS` | deployment-wide: keys and discovery fresh for 300 to 86400 s, default 3600; a shorter `max-age` wins |
/// | `_PRESENCE` | `off` (default) or `on`: `on` counts this connection's two-step claim as presence everywhere; under `off` only a tenant that accepts it (§8.2) |
/// | `_PRESENCE_AMR` | `amr` words that mean two steps, default `mfa` |
/// | `_PRESENCE_ACR` | `acr` values that mean two steps, default none |
/// | `ORDO_AUTH_UPSTREAM_PRESENCE_SECS` | deployment-wide: how old `auth_time` may be, 30 to 300 s, default 300 |
///
/// `ORDO_AUTH_OIDC_CACHE_SECS`, deployment-wide: how long keys and discovery
/// stay fresh. It reaches the deployment's connections and every tenant's
/// own (audit 2026-10-11, finding 18). The error leaves the name to the
/// caller.
pub fn cache_secs_from_vars(var: &dyn Fn(&str) -> Option<String>) -> Result<u64, String> {
    match var(CACHE_ENV).map(|v| v.trim().to_string()) {
        None => Ok(DEFAULT_CACHE_SECS),
        Some(v) if v.is_empty() => Ok(DEFAULT_CACHE_SECS),
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|s| (MIN_CACHE_SECS..=MAX_CACHE_SECS).contains(s))
            .ok_or_else(|| {
                format!(
                    "`{v}` is not a whole number from {MIN_CACHE_SECS} to {MAX_CACHE_SECS}; \
                     unset it for the default, {DEFAULT_CACHE_SECS}"
                )
            }),
    }
}

/// Every error names a setting and never a value.
pub fn connection_from_vars(
    name: &str,
    var: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&str) -> Result<String, String>,
    public_url: &str,
    returns: &ordo_identity::origin::OriginList,
) -> Result<OidcConnection, String> {
    if !valid_connection_name(name) {
        return Err(format!(
            "`{name}` is not a connection name: 1 to 32 of a-z, 0-9 and -"
        ));
    }
    let p = prefix(name);
    let get = |k: &str| {
        var(&format!("{p}{k}"))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let preset = match get("PRESET") {
        Some(word) => Some(oidc_preset(&word).ok_or_else(|| {
            format!(
                "{p}PRESET `{word}` is not an OIDC preset ({})",
                OIDC_PRESETS.join(", ")
            )
        })?),
        None => None,
    };
    match get("KIND").as_deref() {
        None if preset.is_some() => {}
        Some("oidc") => {}
        Some(other) => {
            return Err(format!(
                "{p}KIND `{other}` is not `oidc`; an OIDC preset needs no kind"
            ))
        }
        None => return Err(format!("{p}PRESET or {p}KIND=oidc is required")),
    }
    let pr = preset.as_ref();
    let issuer = match (get("ISSUER"), pr) {
        (Some(i), _) => i,
        (None, Some(x)) if x.needs_tenant => {
            let tenant = get("TENANT").ok_or_else(|| {
                format!("{p}TENANT is required: the Microsoft Entra directory (tenant) id, a GUID")
            })?;
            microsoft_issuer(&tenant).ok_or_else(|| {
                format!(
                    "{p}TENANT must be the directory (tenant) id, a GUID such as \
                     9188040d-6c67-4c5b-b112-36a304b66dad. `common`, `organizations`, \
                     `consumers` and domain names are refused: a connection names one tenant"
                )
            })?
        }
        (None, Some(x)) => x
            .issuer
            .map(str::to_string)
            .ok_or_else(|| format!("{p}ISSUER is required"))?,
        (None, None) => return Err(format!("{p}ISSUER is required")),
    };
    let client_id = get("CLIENT_ID").ok_or_else(|| format!("{p}CLIENT_ID is required"))?;
    let secret_file =
        get("CLIENT_SECRET_FILE").ok_or_else(|| format!("{p}CLIENT_SECRET_FILE is required"))?;
    if var(&format!("{p}CLIENT_SECRET")).is_some() {
        return Err(format!(
            "{p}CLIENT_SECRET is not read: a secret in a variable's value shows in a process \
             listing; put it in a file and name the file in {p}CLIENT_SECRET_FILE"
        ));
    }
    let secret =
        read_file(&secret_file).map_err(|_| format!("{p}CLIENT_SECRET_FILE could not be read"))?;
    let endpoints = match (get("AUTHORIZE_URL"), get("TOKEN_URL"), get("JWKS_URL")) {
        (None, None, None) => None,
        (Some(a), Some(t), Some(j)) => Some(OidcEndpoints {
            authorize_url: a,
            token_url: t,
            jwks_url: j,
            iss_parameter_required: match get("ISS_PARAMETER").as_deref() {
                None => false,
                Some("required") => true,
                Some(_) => return Err(format!("{p}ISS_PARAMETER is `required` or unset")),
            },
        }),
        _ => {
            return Err(format!(
                "{p}AUTHORIZE_URL, {p}TOKEN_URL and {p}JWKS_URL are set together or not at \
                 all; unset all three to use discovery"
            ))
        }
    };
    if endpoints.is_none() && get("ISS_PARAMETER").is_some() {
        return Err(format!(
            "{p}ISS_PARAMETER applies to pinned endpoints; with discovery the provider's \
             document decides it"
        ));
    }
    let use_email = match get("USE_EMAIL").as_deref() {
        None => pr.is_none_or(|x| x.use_email),
        Some("on") => true,
        Some("off") => false,
        Some(_) => {
            return Err(format!(
                "{p}USE_EMAIL is `on` or `off` (default: on, or the preset's own value)"
            ))
        }
    };
    let display_claims = match get("DISPLAY_CLAIMS") {
        Some(list) => list
            .split(',')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect(),
        None => pr
            .map(|x| x.display_claims.to_vec())
            .unwrap_or(PLAIN_DISPLAY_CLAIMS.to_vec())
            .into_iter()
            .map(str::to_string)
            .collect(),
    };
    let d = defaults_from_vars(var).map_err(|(name, d)| format!("{name}: {d}"))?;
    let skew_secs = match get("CLOCK_SKEW_SECS") {
        None => d.skew_secs,
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|s| *s <= MAX_SKEW_SECS)
            .ok_or_else(|| {
                format!(
                    "{p}CLOCK_SKEW_SECS is 0 to {MAX_SKEW_SECS}; unset it for the deployment's, \
                     {SKEW_ENV}={}",
                    d.skew_secs
                )
            })?,
    };
    let cache_secs = d.cache_secs;
    let algs = match get("ALGS") {
        None => d.algs.clone(),
        Some(list) => parse_alg_list(&list).map_err(|e| {
            format!(
                "{p}ALGS is a list of RS256, PS256, ES256 and EdDSA ({}); unset it for the \
                 deployment's {ALGS_ENV} (all four by default). `none` and HS* are never \
                 accepted",
                e.0
            )
        })?,
    };
    let presence = presence_rule(&p, &get, var)?;
    let conn = OidcConnection {
        name: name.to_string(),
        issuer,
        client_id,
        client_secret: ClientSecret::new(secret),
        scope: get("SCOPE")
            .unwrap_or_else(|| pr.map_or("openid email profile", |x| x.scope).to_string()),
        endpoints,
        use_email,
        display_claims,
        avatar_claim: get("AVATAR_CLAIM")
            .or_else(|| pr.and_then(|x| x.avatar_claim).map(str::to_string)),
        skew_secs,
        algs,
        cache_secs,
        stale_grace_secs: d.stale_grace_secs,
        redirect_uri: redirect_uri(public_url, name),
        returns: returns.clone(),
        presence,
    };
    conn.check()
        .map_err(|e| format!("connection `{name}`: {e}"))?;
    HttpsUrl::parse(&conn.discovery_url())
        .map_err(|e| format!("{p}ISSUER: {}", e.disclosable()))?;
    if let Some(e) = &conn.endpoints {
        for (k, url) in [
            ("AUTHORIZE_URL", &e.authorize_url),
            ("TOKEN_URL", &e.token_url),
            ("JWKS_URL", &e.jwks_url),
        ] {
            HttpsUrl::parse(url).map_err(|e| format!("{p}{k}: {}", e.disclosable()))?;
        }
    }
    Ok(conn)
}

/// How old a provider's `auth_time` may be for its two-step claim to count,
/// for every connection. 30 to 300 seconds, default 300: never longer than
/// the access engine's own step-up window.
pub const UPSTREAM_SECS_ENV: &str = "ORDO_AUTH_UPSTREAM_PRESENCE_SECS";

/// The deployment-wide freshness window for a provider's two-step claim.
pub fn upstream_secs(var: &dyn Fn(&str) -> Option<String>) -> Result<u64, String> {
    use ordo_identity::upstream::{DEFAULT_MAX_AGE_SECS, MIN_MAX_AGE_SECS};
    match var(UPSTREAM_SECS_ENV).map(|v| v.trim().to_string()) {
        None => Ok(DEFAULT_MAX_AGE_SECS),
        Some(v) if v.is_empty() => Ok(DEFAULT_MAX_AGE_SECS),
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|s| (MIN_MAX_AGE_SECS..=DEFAULT_MAX_AGE_SECS).contains(s))
            .ok_or_else(|| {
                format!(
                    "{UPSTREAM_SECS_ENV} is {MIN_MAX_AGE_SECS} to {DEFAULT_MAX_AGE_SECS}; unset \
                     it for {DEFAULT_MAX_AGE_SECS}"
                )
            }),
    }
}

/// The setting that lets connection `name`'s two-step claim count, and the
/// value that does: what a refusal names.
pub fn presence_setting(name: &str) -> String {
    format!("{}PRESENCE", prefix(name))
}

/// `_PRESENCE`, `_PRESENCE_AMR`, `_PRESENCE_ACR`. Off unless named.
fn presence_rule(
    p: &str,
    get: &dyn Fn(&str) -> Option<String>,
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<ordo_identity::PresenceRule, String> {
    use ordo_identity::upstream::{PresenceMode, PresenceRule};
    let mode = match get("PRESENCE") {
        None => PresenceMode::Off,
        Some(w) => PresenceMode::parse(&w)
            .ok_or_else(|| format!("{p}PRESENCE is `off` or `on`; unset it for off"))?,
    };
    let words = |k: &str| -> Result<Option<Vec<String>>, String> {
        match get(k) {
            None => Ok(None),
            Some(list) => PresenceRule::parse_words(&list)
                .map(Some)
                .map_err(|e| format!("{p}{k}: {e}")),
        }
    };
    let d = PresenceRule::default();
    let amr = words("PRESENCE_AMR")?.unwrap_or(d.amr);
    let acr = words("PRESENCE_ACR")?.unwrap_or(d.acr);
    // Under `off` a tenant may still accept the claim, so the words matter
    // whatever the mode.
    if amr.is_empty() && acr.is_empty() {
        return Err(format!(
            "{p}PRESENCE_AMR and {p}PRESENCE_ACR name nothing, so no claim could count; unset \
             {p}PRESENCE_AMR for `mfa`"
        ));
    }
    Ok(PresenceRule {
        mode,
        amr,
        acr,
        max_age_secs: upstream_secs(var)?,
    })
}

/// Transports over the egress kernel, each with exactly the hosts asked for
/// as its allowlist.
pub struct EgressFactory {
    roots: TlsRoots,
    deny: DenyList,
}

impl EgressFactory {
    pub fn new(roots: TlsRoots, deny: DenyList) -> EgressFactory {
        EgressFactory { roots, deny }
    }

    /// The deployment's trust anchors (the public root program, plus
    /// `ORDO_AUTH_PROVIDER_CA_FILE` when set) and its deny list.
    pub fn deployment(roots: TlsRoots, deny: DenyList) -> Arc<EgressFactory> {
        Arc::new(EgressFactory::new(roots, deny))
    }
}

impl TransportFactory for EgressFactory {
    fn for_hosts(&self, hosts: &[String]) -> Arc<dyn FederationTransport> {
        Arc::new(EgressTransport::new(
            self.roots.clone(),
            self.deny.clone(),
            hosts,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    fn secret(_: &str) -> Result<String, String> {
        Ok("the-secret\n".into())
    }

    fn returns() -> ordo_identity::origin::OriginList {
        crate::federation::parse_return_urls("https://app.example.com/in", false).unwrap()
    }

    fn build(pairs: &[(&str, &str)], name: &str) -> Result<OidcConnection, String> {
        connection_from_vars(
            name,
            &vars(pairs),
            &secret,
            "https://api.example.com",
            &returns(),
        )
    }

    /// The deployment-wide cache setting is read on its own, so a tenant's
    /// connection gets it too (audit 2026-10-11, finding 18).
    #[test]
    fn deployment_skew_and_algs_are_the_default_a_connection_overrides() {
        let base = [
            ("ORDO_AUTH_CORP_KIND", "oidc"),
            ("ORDO_AUTH_CORP_ISSUER", "https://id.example.com"),
            ("ORDO_AUTH_CORP_CLIENT_ID", "c"),
            ("ORDO_AUTH_CORP_CLIENT_SECRET_FILE", "/s"),
            (SKEW_ENV, "30"),
            (ALGS_ENV, "ES256"),
            (STALE_GRACE_ENV, "600"),
        ];
        let c = build(&base, "corp").unwrap();
        assert_eq!(c.skew_secs, 30);
        assert_eq!(c.algs, vec![Alg::ES256]);
        assert_eq!(c.stale_grace_secs, 600);
        let mut own = base.to_vec();
        own.push(("ORDO_AUTH_CORP_CLOCK_SKEW_SECS", "90"));
        own.push(("ORDO_AUTH_CORP_ALGS", "RS256"));
        let c = build(&own, "corp").unwrap();
        assert_eq!((c.skew_secs, c.algs), (90, vec![Alg::RS256]));
        for (name, bad, range, default) in [
            (SKEW_ENV, "121", "from 0 to 120", "default, 60"),
            (
                STALE_GRACE_ENV,
                "604801",
                "from 0 to 604800",
                "default, 86400",
            ),
        ] {
            let (var, d) = defaults_from_vars(&vars(&[(name, bad)])).unwrap_err();
            assert_eq!(var, name);
            assert!(d.contains(range) && d.contains(default), "{d}");
        }
        let (var, _) = defaults_from_vars(&vars(&[(ALGS_ENV, "HS256")])).unwrap_err();
        assert_eq!(var, ALGS_ENV);
    }

    #[test]
    fn the_cache_setting_is_read_once_for_every_connection() {
        assert_eq!(
            cache_secs_from_vars(&vars(&[])).unwrap(),
            DEFAULT_CACHE_SECS
        );
        assert_eq!(
            cache_secs_from_vars(&vars(&[(CACHE_ENV, "900")])).unwrap(),
            900
        );
        let e = cache_secs_from_vars(&vars(&[(CACHE_ENV, "10")])).unwrap_err();
        assert!(
            e.contains("from 300 to 86400") && e.contains("default, 3600"),
            "{e}"
        );
        assert!(!e.contains(CACHE_ENV), "the caller names it: {e}");
    }

    #[test]
    fn google_is_a_preset_a_client_id_and_a_secret_file() {
        let c = build(
            &[
                ("ORDO_AUTH_GOOGLE_PRESET", "google"),
                ("ORDO_AUTH_GOOGLE_CLIENT_ID", "1.apps.googleusercontent.com"),
                ("ORDO_AUTH_GOOGLE_CLIENT_SECRET_FILE", "/run/secrets/google"),
            ],
            "google",
        )
        .expect("a connection");
        assert_eq!(c.issuer, "https://accounts.google.com");
        assert!(c.use_email);
        assert_eq!(c.avatar_claim.as_deref(), Some("picture"));
        assert_eq!(
            c.redirect_uri,
            "https://api.example.com/auth/callback/google"
        );
        assert!(c.endpoints.is_none(), "endpoints come from discovery");
        assert!(is_oidc(
            "google",
            &vars(&[("ORDO_AUTH_GOOGLE_PRESET", "google")])
        ));
        assert!(!is_oidc(
            "discord",
            &vars(&[("ORDO_AUTH_DISCORD_PRESET", "discord")])
        ));
    }

    #[test]
    fn microsoft_needs_one_tenant_id_and_keeps_no_email() {
        let base = [
            ("ORDO_AUTH_WORK_PRESET", "microsoft"),
            ("ORDO_AUTH_WORK_CLIENT_ID", "c"),
            ("ORDO_AUTH_WORK_CLIENT_SECRET_FILE", "/s"),
        ];
        let e = build(&base, "work").err().expect("refused");
        assert!(e.contains("ORDO_AUTH_WORK_TENANT"), "{e}");
        for bad in ["common", "organizations", "contoso.onmicrosoft.com"] {
            let mut v = base.to_vec();
            v.push(("ORDO_AUTH_WORK_TENANT", bad));
            let e = build(&v, "work").err().expect("refused");
            assert!(e.contains("ORDO_AUTH_WORK_TENANT"), "{e}");
        }
        let mut v = base.to_vec();
        v.push((
            "ORDO_AUTH_WORK_TENANT",
            "9188040d-6c67-4c5b-b112-36a304b66dad",
        ));
        let c = build(&v, "work").unwrap();
        assert_eq!(
            c.issuer,
            "https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0"
        );
        assert!(!c.use_email);
        // An issuer written by hand cannot reach the multi-tenant endpoint
        // either.
        let e = build(
            &[
                ("ORDO_AUTH_CORP_KIND", "oidc"),
                (
                    "ORDO_AUTH_CORP_ISSUER",
                    "https://login.microsoftonline.com/common/v2.0",
                ),
                ("ORDO_AUTH_CORP_CLIENT_ID", "c"),
                ("ORDO_AUTH_CORP_CLIENT_SECRET_FILE", "/s"),
            ],
            "corp",
        )
        .err()
        .expect("refused");
        assert!(e.contains("one concrete issuer"), "{e}");
    }

    #[test]
    fn a_company_login_names_its_issuer_and_may_pin_its_endpoints() {
        let base = [
            ("ORDO_AUTH_CORP_KIND", "oidc"),
            (
                "ORDO_AUTH_CORP_ISSUER",
                "https://sso.example.com/realms/staff",
            ),
            ("ORDO_AUTH_CORP_CLIENT_ID", "ordo"),
            ("ORDO_AUTH_CORP_CLIENT_SECRET_FILE", "/s"),
        ];
        let c = build(&base, "corp").unwrap();
        assert_eq!(
            c.discovery_url(),
            "https://sso.example.com/realms/staff/.well-known/openid-configuration"
        );
        assert_eq!(c.display_claims, vec!["name", "preferred_username"]);
        let mut v = base.to_vec();
        v.push(("ORDO_AUTH_CORP_TOKEN_URL", "https://sso.example.com/t"));
        let e = build(&v, "corp").err().expect("refused");
        assert!(e.contains("set together"), "{e}");
        v.push(("ORDO_AUTH_CORP_AUTHORIZE_URL", "https://sso.example.com/a"));
        v.push(("ORDO_AUTH_CORP_JWKS_URL", "https://sso.example.com/k"));
        v.push(("ORDO_AUTH_CORP_ISS_PARAMETER", "required"));
        let c = build(&v, "corp").unwrap();
        assert!(c.endpoints.unwrap().iss_parameter_required);
    }

    #[test]
    fn refusals_name_the_setting_and_never_the_secret() {
        let base = [
            ("ORDO_AUTH_CORP_KIND", "oidc"),
            ("ORDO_AUTH_CORP_ISSUER", "https://sso.example.com"),
            ("ORDO_AUTH_CORP_CLIENT_ID", "ordo"),
            ("ORDO_AUTH_CORP_CLIENT_SECRET_FILE", "/s"),
        ];
        for (k, v, want) in [
            (
                "ORDO_AUTH_CORP_CLIENT_SECRET",
                "hunter2",
                "CLIENT_SECRET_FILE",
            ),
            ("ORDO_AUTH_CORP_ALGS", "RS256,HS256", "ORDO_AUTH_CORP_ALGS"),
            ("ORDO_AUTH_CORP_CLOCK_SKEW_SECS", "300", "CLOCK_SKEW_SECS"),
            ("ORDO_AUTH_CORP_USE_EMAIL", "yes", "USE_EMAIL"),
            (
                "ORDO_AUTH_OIDC_CACHE_SECS",
                "10",
                "ORDO_AUTH_OIDC_CACHE_SECS",
            ),
            ("ORDO_AUTH_CORP_SCOPE", "email", "openid"),
        ] {
            let mut pairs = base.to_vec();
            pairs.push((k, v));
            let e = build(&pairs, "corp").err().expect("refused");
            assert!(e.contains(want) && !e.contains("hunter2"), "{k}: {e}");
        }
        let mut pairs = base.to_vec();
        pairs[1] = ("ORDO_AUTH_CORP_ISSUER", "http://sso.example.com");
        assert!(build(&pairs, "corp")
            .err()
            .expect("refused")
            .contains("https"));
    }

    const CORP: &[(&str, &str)] = &[
        ("ORDO_AUTH_CORP_KIND", "oidc"),
        ("ORDO_AUTH_CORP_ISSUER", "https://login.corp.example"),
        ("ORDO_AUTH_CORP_CLIENT_ID", "ordo"),
        ("ORDO_AUTH_CORP_CLIENT_SECRET_FILE", "/run/secrets/corp"),
    ];

    fn corp(extra: &[(&str, &str)]) -> Result<OidcConnection, String> {
        let mut all = CORP.to_vec();
        all.extend_from_slice(extra);
        build(&all, "corp")
    }

    /// Step 10: a connection's two-step claim counts nowhere unless the app
    /// developer says so, and a wrong value names the setting.
    #[test]
    fn the_presence_setting_is_off_by_default_and_names_itself_when_wrong() {
        use ordo_identity::upstream::{PresenceMode, DEFAULT_AMR, DEFAULT_MAX_AGE_SECS};
        let c = corp(&[]).expect("a connection");
        assert_eq!(c.presence.mode, PresenceMode::Off);
        assert_eq!(c.presence.amr, vec![DEFAULT_AMR.to_string()]);
        assert!(c.presence.acr.is_empty());
        assert_eq!(c.presence.max_age_secs, DEFAULT_MAX_AGE_SECS);
        assert_eq!(presence_setting("corp"), "ORDO_AUTH_CORP_PRESENCE");

        let c = corp(&[
            ("ORDO_AUTH_CORP_PRESENCE", "on"),
            ("ORDO_AUTH_CORP_PRESENCE_AMR", "mfa,hwk"),
            ("ORDO_AUTH_CORP_PRESENCE_ACR", "urn:corp:loa:2"),
            (UPSTREAM_SECS_ENV, "120"),
        ])
        .expect("a connection");
        assert_eq!(c.presence.mode, PresenceMode::On);
        assert_eq!(c.presence.amr, vec!["mfa".to_string(), "hwk".to_string()]);
        assert_eq!(c.presence.acr, vec!["urn:corp:loa:2".to_string()]);
        assert_eq!(c.presence.max_age_secs, 120);

        for (extra, names) in [
            (
                vec![("ORDO_AUTH_CORP_PRESENCE", "yes")],
                "ORDO_AUTH_CORP_PRESENCE",
            ),
            (
                vec![("ORDO_AUTH_CORP_PRESENCE", "tenant")],
                "ORDO_AUTH_CORP_PRESENCE",
            ),
            (
                vec![("ORDO_AUTH_CORP_PRESENCE_AMR", ",")],
                "ORDO_AUTH_CORP_PRESENCE_AMR",
            ),
            (vec![(UPSTREAM_SECS_ENV, "3600")], UPSTREAM_SECS_ENV),
            (
                vec![
                    ("ORDO_AUTH_CORP_PRESENCE", "on"),
                    ("ORDO_AUTH_CORP_PRESENCE_AMR", ","),
                ],
                "ORDO_AUTH_CORP_PRESENCE_AMR",
            ),
            (
                vec![("ORDO_AUTH_CORP_PRESENCE_AMR", "m f a")],
                "ORDO_AUTH_CORP_PRESENCE_AMR",
            ),
        ] {
            let e = corp(&extra).err().expect("refused");
            assert!(e.contains(names), "{e}");
        }
    }
}
