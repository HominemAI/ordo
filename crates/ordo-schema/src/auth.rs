//! The `auth` block, checked and resolved
//! (`plan-configuration-in-the-file.md` §2.2 and §2.3).
//!
//! The file sets the app developer's value. The deployment owner's
//! ceilings narrow it at run time and are not read here. What is checked
//! is what the block alone can answer.
//!
//! | Class | Refused |
//! |---|---|
//! | `auth_unknown_method` | a method that is not built in and names no `provider` in the block |
//! | `auth_unused_provider` | a `provider` no `methods` line names: it would never show |
//! | `auth_unknown_preset` | a preset the host does not ship |
//! | `auth_duration_range` | a duration outside the platform range; the message names the range |
//! | `invalid_auth` | a method listed twice, a provider declared twice or named like a built-in method, a domain that is not a DNS name or is listed twice, an empty `passkeyName`, a session `max` below its `token`, a `tenants` range that runs backwards |
//!
//! A second block is refused by the parser (`duplicate_auth`).
//!
//! [`AuthValues::resolve`] fills every absent clause with its secure
//! default. The compiled program carries those values, so a reader of the
//! artifact never has to know the defaults.

use ordo_syntax::{
    AuthDecl, AuthDuration, Provisioning, RememberMax, SecondFactor, Span, UserVerification,
    AUTH_BUILT_IN_METHODS, AUTH_PRESETS,
};

use crate::SchemaError;

/// One platform range, inclusive, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub min: u64,
    pub max: u64,
}

const DAY: u64 = 86_400;

/// `linking on, window …`: the host's `ORDO_AUTH_LINKING_SECS` range.
pub const LINKING_WINDOW: Range = Range { min: 60, max: 900 };
/// `guest on, keep …`. A guest can be kept no longer than the longest a
/// browser keeps a cookie. The deployment's registration lifetime still
/// caps it at run time.
pub const GUEST_KEEP: Range = Range {
    min: 60,
    max: 400 * DAY,
};
/// `session token …` and both ends of `tenants`: the host's token floor
/// and its default token ceiling (`session_policy::TOKEN_FLOOR`,
/// `DEFAULT_TOKEN_LIMIT`).
pub const SESSION_TOKEN: Range = Range { min: 60, max: DAY };
/// `session …, max …`: up to the host's default ceiling, 30 days.
pub const SESSION_MAX: Range = Range {
    min: 60,
    max: 30 * DAY,
};
/// `remember on, idle …` and `max …`: up to 400 days, the longest a
/// browser keeps a cookie (`session_policy::DEFAULT_REMEMBER_LIMIT`).
pub const REMEMBER: Range = Range {
    min: 60,
    max: 400 * DAY,
};

/// The defaults when a clause is absent. Each is the secure one (§2.3).
pub const DEFAULT_LINKING_WINDOW_SECS: u64 = 300;
pub const DEFAULT_SESSION_TOKEN_SECS: u64 = 3_600;
pub const DEFAULT_SESSION_MAX_SECS: u64 = 12 * 3_600;
pub const DEFAULT_REMEMBER_IDLE_SECS: u64 = 30 * DAY;

/// A provider, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderValues {
    pub name: String,
    /// `None` for `kind oauth2` or `kind oidc` written out.
    pub preset: Option<String>,
    /// `oauth2` or `oidc`.
    pub kind: &'static str,
    pub label: Option<String>,
}

/// The block's values with every default filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthValues {
    /// In the order written. Empty: nobody signs in.
    pub methods: Vec<String>,
    pub providers: Vec<ProviderValues>,
    /// `None`: the relying party id, which is the deployment's.
    pub passkey_name: Option<String>,
    /// `invite-only`, `domains` or `open`.
    pub provisioning: &'static str,
    /// Lowercased, in the order written. Empty unless `domains`.
    pub domains: Vec<String>,
    pub self_serve: bool,
    pub guest: bool,
    /// `None`: as long as any registration.
    pub guest_keep_secs: Option<u64>,
    pub linking: bool,
    pub linking_window_secs: u64,
    pub second_factor: &'static str,
    pub user_verification: &'static str,
    pub session_token_secs: u64,
    pub session_max_secs: u64,
    /// The range a tenant may choose its token life in. `None`: inside
    /// the deployment's ceilings.
    pub tenant_token_secs: Option<(u64, u64)>,
    pub remember: bool,
    pub remember_idle_secs: u64,
    /// `None`: no overall limit.
    pub remember_max_secs: Option<u64>,
}

impl AuthValues {
    /// Resolve a checked block. Call [`check`] first: an unchecked block
    /// may carry a duration that overflows, read here as the range's top.
    pub fn resolve(a: &AuthDecl) -> AuthValues {
        let secs = |d: &AuthDuration| d.secs().unwrap_or(u64::MAX);
        let (provisioning, domains) = match a.provisioning.as_ref().map(|p| &p.mode) {
            None | Some(Provisioning::InviteOnly) => ("invite-only", Vec::new()),
            Some(Provisioning::Open) => ("open", Vec::new()),
            Some(Provisioning::Domains(ds)) => {
                ("domains", ds.iter().map(|d| d.name.clone()).collect())
            }
        };
        let guest = a.guest.as_ref();
        let linking = a.linking.as_ref();
        let session = a.session.as_ref();
        let remember = a.remember.as_ref();
        AuthValues {
            methods: a
                .methods
                .as_ref()
                .map(|m| m.names.iter().map(|n| n.name.clone()).collect())
                .unwrap_or_default(),
            providers: a
                .providers
                .iter()
                .map(|p| ProviderValues {
                    name: p.name.name.clone(),
                    preset: match &p.source {
                        ordo_syntax::ProviderSource::Preset(i) => Some(i.name.clone()),
                        ordo_syntax::ProviderSource::Kind(_) => None,
                    },
                    kind: p.kind().map_or("oauth2", |k| k.as_str()),
                    label: p.label.clone(),
                })
                .collect(),
            passkey_name: a.passkey_name.as_ref().map(|t| t.text.clone()),
            provisioning,
            domains,
            self_serve: a.self_serve.as_ref().is_some_and(|s| s.on),
            guest: guest.is_some_and(|g| g.on),
            guest_keep_secs: guest.and_then(|g| g.keep.as_ref()).map(secs),
            linking: linking.is_none_or(|l| l.on),
            linking_window_secs: linking
                .and_then(|l| l.window.as_ref())
                .map_or(DEFAULT_LINKING_WINDOW_SECS, secs),
            second_factor: a
                .second_factor
                .as_ref()
                .map_or(SecondFactor::Optional, |s| s.level)
                .as_str(),
            user_verification: a
                .user_verification
                .as_ref()
                .map_or(UserVerification::Required, |u| u.level)
                .as_str(),
            session_token_secs: session.map_or(DEFAULT_SESSION_TOKEN_SECS, |s| secs(&s.token)),
            session_max_secs: session
                .and_then(|s| s.max.as_ref())
                .map_or(DEFAULT_SESSION_MAX_SECS, secs),
            tenant_token_secs: session
                .and_then(|s| s.tenants.as_ref())
                .map(|t| (secs(&t.min), secs(&t.max))),
            remember: remember.is_none_or(|r| r.on),
            remember_idle_secs: remember
                .and_then(|r| r.idle.as_ref())
                .map_or(DEFAULT_REMEMBER_IDLE_SECS, secs),
            remember_max_secs: match remember.and_then(|r| r.max.as_ref()) {
                None | Some(RememberMax::Unbounded { .. }) => None,
                Some(RememberMax::After(d)) => Some(secs(d)),
            },
        }
    }
}

fn invalid(span: Span, message: String) -> SchemaError {
    SchemaError {
        code: "invalid_auth",
        message,
        span,
    }
}

/// `90000` as `25h`, `604800` as `7d`: the largest whole unit.
fn spell_secs(s: u64) -> String {
    for (per, unit) in [(7 * DAY, "w"), (DAY, "d"), (3_600, "h"), (60, "m")] {
        if s % per == 0 && s >= per {
            return format!("{}{unit}", s / per);
        }
    }
    format!("{s}s")
}

fn in_range(d: &AuthDuration, what: &str, r: Range) -> Result<u64, SchemaError> {
    match d.secs() {
        Some(s) if (r.min..=r.max).contains(&s) => Ok(s),
        _ => Err(SchemaError {
            code: "auth_duration_range",
            message: format!(
                "`{what} {}` is outside the platform range, {} to {}",
                d.spelled(),
                spell_secs(r.min),
                spell_secs(r.max)
            ),
            span: d.span,
        }),
    }
}

/// An email domain: a DNS name with at least two labels.
fn domain_ok(d: &str) -> bool {
    let labels: Vec<&str> = d.split('.').collect();
    d.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Check the block.
pub fn check(a: &AuthDecl) -> Result<(), SchemaError> {
    // Providers first: a method's refusal can then say which exist.
    let mut seen: Vec<&str> = Vec::new();
    for p in &a.providers {
        let name = p.name.name.as_str();
        if AUTH_BUILT_IN_METHODS.contains(&name) {
            return Err(invalid(
                p.name.span,
                format!("`{name}` is a built-in method; give the provider another name"),
            ));
        }
        if seen.contains(&name) {
            return Err(invalid(
                p.span,
                format!("provider `{name}` is declared twice"),
            ));
        }
        seen.push(name);
        if let ordo_syntax::ProviderSource::Preset(preset) = &p.source {
            if !AUTH_PRESETS.iter().any(|(n, _)| *n == preset.name) {
                let list = AUTH_PRESETS
                    .iter()
                    .map(|(n, _)| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(SchemaError {
                    code: "auth_unknown_preset",
                    message: format!(
                        "provider `{name}`: `{}` is not a preset the host ships ({list}); for \
                         another provider write `kind oauth2` or `kind oidc`",
                        preset.name
                    ),
                    span: preset.span,
                });
            }
        }
    }

    let methods = a
        .methods
        .as_ref()
        .map(|m| m.names.as_slice())
        .unwrap_or(&[]);
    let mut listed: Vec<&str> = Vec::new();
    for m in methods {
        let name = m.name.as_str();
        if listed.contains(&name) {
            return Err(invalid(m.span, format!("method `{name}` is listed twice")));
        }
        listed.push(name);
        if !AUTH_BUILT_IN_METHODS.contains(&name) && !seen.contains(&name) {
            return Err(SchemaError {
                code: "auth_unknown_method",
                message: format!(
                    "method `{name}` is not built in (`passkey`, `password`, `email`) and no \
                     provider has that name; declare it: `provider {name} preset {name}` or \
                     `provider {name} kind oidc`"
                ),
                span: m.span,
            });
        }
    }
    for p in &a.providers {
        if !listed.contains(&p.name.name.as_str()) {
            return Err(SchemaError {
                code: "auth_unused_provider",
                message: format!(
                    "provider `{}` is in no `methods` line, so nobody could use it; add it to \
                     `methods` or remove it",
                    p.name.name
                ),
                span: p.span,
            });
        }
    }

    if let Some(t) = &a.passkey_name {
        if t.text.trim().is_empty() {
            return Err(invalid(
                t.span,
                "`passkeyName` is the name a person sees on their passkey; it cannot be empty"
                    .into(),
            ));
        }
    }

    if let Some(Provisioning::Domains(ds)) = a.provisioning.as_ref().map(|p| &p.mode) {
        let mut names: Vec<&str> = Vec::new();
        for d in ds {
            if !domain_ok(&d.name) {
                return Err(invalid(
                    d.span,
                    format!("`{}` is not an email domain such as `example.com`", d.name),
                ));
            }
            if names.contains(&d.name.as_str()) {
                return Err(invalid(
                    d.span,
                    format!("domain `{}` is listed twice", d.name),
                ));
            }
            names.push(&d.name);
        }
    }

    if let Some(g) = a.guest.as_ref().and_then(|g| g.keep.as_ref()) {
        in_range(g, "keep", GUEST_KEEP)?;
    }
    if let Some(w) = a.linking.as_ref().and_then(|l| l.window.as_ref()) {
        in_range(w, "window", LINKING_WINDOW)?;
    }
    if let Some(s) = &a.session {
        let token = in_range(&s.token, "token", SESSION_TOKEN)?;
        if let Some(m) = &s.max {
            let max = in_range(m, "max", SESSION_MAX)?;
            if max < token {
                return Err(invalid(
                    m.span,
                    format!(
                        "a session `max` of {} is below its `token` of {}; every token would \
                         be cut to the max",
                        m.spelled(),
                        s.token.spelled()
                    ),
                ));
            }
        }
        if let Some(t) = &s.tenants {
            let lo = in_range(&t.min, "tenants", SESSION_TOKEN)?;
            let hi = in_range(&t.max, "tenants", SESSION_TOKEN)?;
            if lo > hi {
                return Err(invalid(
                    t.span,
                    format!(
                        "`tenants {} to {}` runs backwards; write the shorter life first",
                        t.min.spelled(),
                        t.max.spelled()
                    ),
                ));
            }
        }
    }
    if let Some(r) = &a.remember {
        if let Some(i) = &r.idle {
            in_range(i, "idle", REMEMBER)?;
        }
        if let Some(RememberMax::After(m)) = &r.max {
            in_range(m, "max", REMEMBER)?;
        }
    }
    Ok(())
}
