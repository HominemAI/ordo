//! The `auth` block: how people sign in to the app
//! (`plan-configuration-in-the-file.md` §2).
//!
//! ```text
//! auth {
//!   methods passkey, email, google, work
//!   provider google preset google
//!   provider work kind oidc, label "Work account"
//!   passkeyName "Human Agent Link"
//!   provisioning domains example.com, example.org
//!   selfServe off
//!   guest on, keep 7d
//!   linking on, window 5m
//!   secondFactor optional
//!   userVerification required
//!   session token 1h, max 12h, tenants 15m to 4h
//!   remember on, idle 30d, max none
//! }
//! ```
//!
//! The file holds the app developer's value. The deployment owner's
//! ceilings stay in the environment and narrow it at run time. Values that
//! cannot change once used (the relying party id, the apex domain, the
//! database) are never here.
//!
//! Inside the braces the lexer emits no line breaks, so each clause is
//! found by its leading word, as in `function`. The parser reads the shape
//! and refuses a clause it cannot read, or a clause written twice.
//! `ordo-schema` checks what the clauses mean together: a method that names
//! no provider, a provider no method uses, an unknown preset, a duration
//! outside the platform range.

use crate::ast::Ident;
use crate::decl::FunctionHost;
use crate::declparse::{function_host, ident};
use crate::parser::{describe, ParseError, Parser};
use crate::span::Span;
use crate::token::{DurationUnit, Keyword, TokenKind};

/// The methods every deployment knows without a `provider` line.
pub const AUTH_BUILT_IN_METHODS: &[&str] = &["passkey", "password", "email"];

/// The provider presets the host ships. A preset fixes the endpoints and
/// the kind; the deployment owner still sets the client id and secret.
/// `discord` and `github` are OAuth 2 presets, `google` and `microsoft`
/// OIDC presets (`ordo_identity::oauth2::PRESETS`, `oidc::OIDC_PRESETS`).
pub const AUTH_PRESETS: &[(&str, ProviderKind)] = &[
    ("discord", ProviderKind::Oauth2),
    ("github", ProviderKind::Oauth2),
    ("google", ProviderKind::Oidc),
    ("microsoft", ProviderKind::Oidc),
];

/// The clause words, in canonical order.
pub const AUTH_CLAUSES: &[&str] = &[
    "methods",
    "provider",
    "passkeyName",
    "provisioning",
    "selfServe",
    "guest",
    "linking",
    "secondFactor",
    "userVerification",
    "session",
    "remember",
];

/// `auth { … }`. Every clause is optional; an absent clause takes the
/// secure default `ordo-schema` states. One block per program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthDecl {
    pub methods: Option<AuthMethods>,
    pub providers: Vec<AuthProvider>,
    pub passkey_name: Option<AuthText>,
    pub provisioning: Option<AuthProvisioning>,
    pub self_serve: Option<AuthSwitch>,
    pub guest: Option<AuthGuest>,
    pub linking: Option<AuthLinking>,
    pub second_factor: Option<AuthSecondFactor>,
    pub user_verification: Option<AuthUserVerification>,
    pub session: Option<AuthSession>,
    pub remember: Option<AuthRemember>,
    pub span: Span,
}

/// `methods passkey, email, google`: in the order written, which is the
/// order a sign-in page offers them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethods {
    pub names: Vec<Ident>,
    pub span: Span,
}

/// `provider work kind oidc, label "Work account"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthProvider {
    pub name: Ident,
    pub source: ProviderSource,
    pub label: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSource {
    /// `preset google`: one the host ships.
    Preset(Ident),
    /// `kind oauth2` or `kind oidc`: the deployment owner sets the endpoints.
    Kind(ProviderKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Oauth2,
    Oidc,
}

impl ProviderKind {
    pub const ALL: &'static [ProviderKind] = &[ProviderKind::Oauth2, ProviderKind::Oidc];

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Oauth2 => "oauth2",
            ProviderKind::Oidc => "oidc",
        }
    }
}

impl AuthProvider {
    /// The provider's kind: written, or fixed by its preset. `None` for a
    /// preset the host does not ship, which `ordo-schema` refuses.
    pub fn kind(&self) -> Option<ProviderKind> {
        match &self.source {
            ProviderSource::Kind(k) => Some(*k),
            ProviderSource::Preset(p) => AUTH_PRESETS
                .iter()
                .find(|(n, _)| *n == p.name)
                .map(|(_, k)| *k),
        }
    }
}

/// A quoted string and where it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthText {
    pub text: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthProvisioning {
    pub mode: Provisioning,
    pub span: Span,
}

/// Who may become a member without an invitation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provisioning {
    /// Nobody: a tenant invites each person. The default.
    InviteOnly,
    /// A stranger whose verified email is at one of these domains.
    Domains(Vec<FunctionHost>),
    /// Anybody who signs in.
    Open,
}

impl Provisioning {
    /// The mode's spelling, without its domains.
    pub fn mode(&self) -> &'static str {
        match self {
            Provisioning::InviteOnly => "invite-only",
            Provisioning::Domains(_) => "domains",
            Provisioning::Open => "open",
        }
    }
}

/// The provisioning modes, loosest last.
pub const PROVISIONING_MODES: &[&str] = &["invite-only", "domains", "open"];

/// `selfServe on` or `selfServe off`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSwitch {
    pub on: bool,
    pub span: Span,
}

/// `guest on, keep 7d`. `keep` is written only after `on`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthGuest {
    pub on: bool,
    pub keep: Option<AuthDuration>,
    pub span: Span,
}

/// `linking on, window 5m`. `window` is written only after `on`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthLinking {
    pub on: bool,
    pub window: Option<AuthDuration>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSecondFactor {
    pub level: SecondFactor,
    pub span: Span,
}

/// How much a second step is asked of a person, loosest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondFactor {
    Off,
    Optional,
    RequiredWithoutPasskey,
    Required,
}

impl SecondFactor {
    pub const ALL: &'static [SecondFactor] = &[
        SecondFactor::Off,
        SecondFactor::Optional,
        SecondFactor::RequiredWithoutPasskey,
        SecondFactor::Required,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SecondFactor::Off => "off",
            SecondFactor::Optional => "optional",
            SecondFactor::RequiredWithoutPasskey => "required-without-passkey",
            SecondFactor::Required => "required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthUserVerification {
    pub level: UserVerification,
    pub span: Span,
}

/// Whether a passkey must check the person (a PIN or a fingerprint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserVerification {
    Required,
    Preferred,
}

impl UserVerification {
    pub const ALL: &'static [UserVerification] =
        &[UserVerification::Required, UserVerification::Preferred];

    pub fn as_str(self) -> &'static str {
        match self {
            UserVerification::Required => "required",
            UserVerification::Preferred => "preferred",
        }
    }
}

/// `session token 1h, max 12h, tenants 15m to 4h`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSession {
    /// How long one token is good for.
    pub token: AuthDuration,
    /// How long a sign-in lasts before the person signs in again.
    pub max: Option<AuthDuration>,
    /// The range a tenant may choose a token life in.
    pub tenants: Option<AuthRange>,
    pub span: Span,
}

/// `15m to 4h`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRange {
    pub min: AuthDuration,
    pub max: AuthDuration,
    pub span: Span,
}

/// `remember on, idle 30d, max none`. `idle` and `max` only after `on`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRemember {
    pub on: bool,
    pub idle: Option<AuthDuration>,
    pub max: Option<RememberMax>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RememberMax {
    /// `max none`: a remembered sign-in lasts while it is used.
    Unbounded { span: Span },
    /// `max 90d`.
    After(AuthDuration),
}

/// A duration as written: `7d`, `5m`. Kept in its unit so a printer
/// writes back what the author wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthDuration {
    pub n: u64,
    pub unit: DurationUnit,
    pub span: Span,
}

impl AuthDuration {
    /// In seconds. `None` on overflow.
    pub fn secs(&self) -> Option<u64> {
        let per = match self.unit {
            DurationUnit::S => 1,
            DurationUnit::M => 60,
            DurationUnit::H => 3_600,
            DurationUnit::D => 86_400,
            DurationUnit::W => 7 * 86_400,
            // Refused by the parser.
            DurationUnit::Ms | DurationUnit::Mo | DurationUnit::Y => return None,
        };
        self.n.checked_mul(per)
    }

    /// As written: `7d`.
    pub fn spelled(&self) -> String {
        format!("{}{}", self.n, self.unit.as_str())
    }
}

// ── parsing ──────────────────────────────────────────────────────────────

fn clause_list() -> String {
    AUTH_CLAUSES
        .iter()
        .map(|c| format!("`{c}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `auth { <clause> … }`. The caller has seen `auth`.
pub(crate) fn auth_decl(p: &mut Parser) -> Result<AuthDecl, ParseError> {
    let start = p.bump().span; // `auth`
    p.expect(&TokenKind::LBrace, "`{` opening the auth block")?;
    let mut a = AuthDecl {
        methods: None,
        providers: Vec::new(),
        passkey_name: None,
        provisioning: None,
        self_serve: None,
        guest: None,
        linking: None,
        second_factor: None,
        user_verification: None,
        session: None,
        remember: None,
        span: start,
    };
    loop {
        let here = p.tok().span;
        let word = match p.peek().clone() {
            TokenKind::RBrace => break,
            TokenKind::Ident(w) => w,
            other => {
                return Err(p.err_here(format!(
                    "expected an auth clause ({}), found {}",
                    clause_list(),
                    describe(&other)
                )))
            }
        };
        let twice = |p: &Parser, set: bool| -> Result<(), ParseError> {
            if set {
                Err(p.err_at(
                    here,
                    format!("the auth block has one `{word}` clause; this is a second"),
                ))
            } else {
                Ok(())
            }
        };
        match word.as_str() {
            "methods" => {
                twice(p, a.methods.is_some())?;
                p.bump();
                let mut names = Vec::new();
                loop {
                    names.push(ident(
                        p,
                        "a sign-in method (`passkey`, `password`, `email` or a provider name)",
                    )?);
                    if !eat_comma(p) {
                        break;
                    }
                }
                a.methods = Some(AuthMethods {
                    names,
                    span: here.to(p.prev_span()),
                });
            }
            "provider" => {
                p.bump();
                a.providers.push(provider(p, here)?);
            }
            "passkeyName" => {
                twice(p, a.passkey_name.is_some())?;
                p.bump();
                let text = match p.peek().clone() {
                    TokenKind::Str(s) => {
                        p.bump();
                        s
                    }
                    other => {
                        return Err(p.err_here(format!(
                            "`passkeyName` is the name a person sees on their passkey, in \
                             quotes: `passkeyName \"My App\"`; found {}",
                            describe(&other)
                        )))
                    }
                };
                a.passkey_name = Some(AuthText {
                    text,
                    span: here.to(p.prev_span()),
                });
            }
            "provisioning" => {
                twice(p, a.provisioning.is_some())?;
                p.bump();
                let w = hyphen_word(p, "`invite-only`, `open` or `domains`")?;
                let mode = match w.name.as_str() {
                    "invite-only" => Provisioning::InviteOnly,
                    "open" => Provisioning::Open,
                    "domains" => {
                        let mut ds = Vec::new();
                        loop {
                            ds.push(function_host(p)?);
                            if !eat_comma(p) {
                                break;
                            }
                        }
                        Provisioning::Domains(ds)
                    }
                    other => {
                        return Err(p.err_at(
                            w.span,
                            format!(
                                "unknown provisioning `{other}`: write `invite-only`, `open`, \
                                 or `domains` and the email domains it admits"
                            ),
                        ))
                    }
                };
                a.provisioning = Some(AuthProvisioning {
                    mode,
                    span: here.to(p.prev_span()),
                });
            }
            "selfServe" => {
                twice(p, a.self_serve.is_some())?;
                p.bump();
                let on = on_off(p, "selfServe")?;
                a.self_serve = Some(AuthSwitch {
                    on,
                    span: here.to(p.prev_span()),
                });
            }
            "guest" => {
                twice(p, a.guest.is_some())?;
                p.bump();
                let on = on_off(p, "guest")?;
                let mut keep = None;
                options(p, on, "guest", &["keep"], |p, opt| {
                    once(p, keep.is_some(), opt)?;
                    keep = Some(duration(p, "keep")?);
                    Ok(())
                })?;
                a.guest = Some(AuthGuest {
                    on,
                    keep,
                    span: here.to(p.prev_span()),
                });
            }
            "linking" => {
                twice(p, a.linking.is_some())?;
                p.bump();
                let on = on_off(p, "linking")?;
                let mut window = None;
                options(p, on, "linking", &["window"], |p, opt| {
                    once(p, window.is_some(), opt)?;
                    window = Some(duration(p, "window")?);
                    Ok(())
                })?;
                a.linking = Some(AuthLinking {
                    on,
                    window,
                    span: here.to(p.prev_span()),
                });
            }
            "secondFactor" => {
                twice(p, a.second_factor.is_some())?;
                p.bump();
                let w = hyphen_word(
                    p,
                    "`off`, `optional`, `required-without-passkey` or `required`",
                )?;
                let Some(level) = SecondFactor::ALL
                    .iter()
                    .copied()
                    .find(|l| l.as_str() == w.name)
                else {
                    return Err(p.err_at(
                        w.span,
                        format!(
                            "unknown secondFactor `{}`: write `off`, `optional`, \
                             `required-without-passkey` or `required`",
                            w.name
                        ),
                    ));
                };
                a.second_factor = Some(AuthSecondFactor {
                    level,
                    span: here.to(p.prev_span()),
                });
            }
            "userVerification" => {
                twice(p, a.user_verification.is_some())?;
                p.bump();
                let w = ident(p, "`required` or `preferred`")?;
                let Some(level) = UserVerification::ALL
                    .iter()
                    .copied()
                    .find(|l| l.as_str() == w.name)
                else {
                    return Err(p.err_at(
                        w.span,
                        format!(
                            "unknown userVerification `{}`: write `required` or `preferred`",
                            w.name
                        ),
                    ));
                };
                a.user_verification = Some(AuthUserVerification {
                    level,
                    span: here.to(p.prev_span()),
                });
            }
            "session" => {
                twice(p, a.session.is_some())?;
                p.bump();
                a.session = Some(session(p, here)?);
            }
            "remember" => {
                twice(p, a.remember.is_some())?;
                p.bump();
                let on = on_off(p, "remember")?;
                let mut idle = None;
                let mut max = None;
                options(p, on, "remember", &["idle", "max"], |p, opt| {
                    match opt.name.as_str() {
                        "idle" => {
                            once(p, idle.is_some(), opt)?;
                            idle = Some(duration(p, "idle")?);
                            Ok(())
                        }
                        _ => {
                            once(p, max.is_some(), opt)?;
                            if matches!(p.peek(), TokenKind::Keyword(Keyword::None)) {
                                let t = p.bump();
                                max = Some(RememberMax::Unbounded { span: t.span });
                            } else {
                                max = Some(RememberMax::After(duration(p, "max")?));
                            }
                            Ok(())
                        }
                    }
                })?;
                a.remember = Some(AuthRemember {
                    on,
                    idle,
                    max,
                    span: here.to(p.prev_span()),
                });
            }
            "requires" | "recovery" | "humanCheck" => {
                return Err(p.err_here(format!(
                    "`{word}` is not part of the auth block yet; the clauses are {}",
                    clause_list()
                )))
            }
            _ => {
                return Err(p.err_here(format!(
                    "unknown auth clause `{word}`; the clauses are {}",
                    clause_list()
                )))
            }
        }
    }
    let end = p.expect(&TokenKind::RBrace, "`}` closing the auth block")?;
    a.span = start.to(end.span);
    Ok(a)
}

fn eat_comma(p: &mut Parser) -> bool {
    if matches!(p.peek(), TokenKind::Comma) {
        p.bump();
        true
    } else {
        false
    }
}

/// Refuse an option written twice in one clause.
fn once(p: &Parser, set: bool, opt: &Ident) -> Result<(), ParseError> {
    if set {
        Err(p.err_at(
            opt.span,
            format!("`{}` is written once in this clause", opt.name),
        ))
    } else {
        Ok(())
    }
}

/// `on` or `off`.
fn on_off(p: &mut Parser, clause: &str) -> Result<bool, ParseError> {
    match p.peek().clone() {
        TokenKind::Keyword(Keyword::On) => {
            p.bump();
            Ok(true)
        }
        TokenKind::Ident(w) if w == "off" => {
            p.bump();
            Ok(false)
        }
        other => Err(p.err_here(format!(
            "`{clause}` is `on` or `off`, found {}",
            describe(&other)
        ))),
    }
}

/// The `, name value` options after `on`. After `off` there are none: a
/// window for linking that is off would say something that never happens.
fn options(
    p: &mut Parser,
    on: bool,
    clause: &str,
    allowed: &[&str],
    mut each: impl FnMut(&mut Parser, &Ident) -> Result<(), ParseError>,
) -> Result<(), ParseError> {
    while matches!(p.peek(), TokenKind::Comma) {
        p.bump();
        let opt = ident(p, &format!("an option of `{clause}`"))?;
        if !allowed.contains(&opt.name.as_str()) {
            let list = allowed
                .iter()
                .map(|a| format!("`{a}`"))
                .collect::<Vec<_>>()
                .join(" and ");
            return Err(p.err_at(
                opt.span,
                format!("`{clause}` takes {list}, not `{}`", opt.name),
            ));
        }
        if !on {
            return Err(p.err_at(
                opt.span,
                format!(
                    "`{clause} off` takes no options; write `{clause} on, {} …` to set one",
                    opt.name
                ),
            ));
        }
        each(p, &opt)?;
    }
    Ok(())
}

/// A word that may carry hyphens: `invite-only`, `required-without-passkey`.
/// The pieces must touch, so `a - b` is not read as one word.
fn hyphen_word(p: &mut Parser, what: &str) -> Result<Ident, ParseError> {
    let first = ident(p, what)?;
    let mut name = first.name;
    let mut span = first.span;
    while matches!(p.peek(), TokenKind::Minus) && p.tok().span.start == span.end {
        let minus = p.bump().span;
        let next = match p.peek().clone() {
            TokenKind::Ident(w) if p.tok().span.start == minus.end => {
                let t = p.bump();
                Ident {
                    name: w,
                    span: t.span,
                }
            }
            other => {
                return Err(p.err_here(format!(
                    "expected {what}, found `{name}-` then {}",
                    describe(&other)
                )))
            }
        };
        name.push('-');
        name.push_str(&next.name);
        span = span.to(next.span);
    }
    Ok(Ident { name, span })
}

/// A duration in seconds, minutes, hours, days or weeks.
fn duration(p: &mut Parser, what: &str) -> Result<AuthDuration, ParseError> {
    match p.peek().clone() {
        TokenKind::Duration { n, unit } => {
            if matches!(unit, DurationUnit::Ms | DurationUnit::Mo | DurationUnit::Y) {
                return Err(p.err_here(format!(
                    "`{what}` is in s, m, h, d or w, such as `{what} 5m`; `{n}{}` is not",
                    unit.as_str()
                )));
            }
            let t = p.bump();
            Ok(AuthDuration {
                n,
                unit,
                span: t.span,
            })
        }
        other => Err(p.err_here(format!(
            "`{what}` is a duration, such as `{what} 5m`; found {}",
            describe(&other)
        ))),
    }
}

/// `provider <name> (preset <p> | kind oauth2 | kind oidc) [, label "<text>"]`.
fn provider(p: &mut Parser, here: Span) -> Result<AuthProvider, ParseError> {
    let name = ident(p, "a provider name, such as `google` or `work`")?;
    let how = match p.peek().clone() {
        TokenKind::Ident(w) if w == "preset" || w == "kind" => {
            p.bump();
            w
        }
        other => {
            return Err(p.err_here(format!(
                "`provider {}` is followed by `preset <name>` or `kind oauth2|oidc`, found {}",
                name.name,
                describe(&other)
            )))
        }
    };
    let source = if how == "preset" {
        ProviderSource::Preset(ident(p, "a preset, such as `google`")?)
    } else {
        let k = ident(p, "`oauth2` or `oidc`")?;
        match k.name.as_str() {
            "oauth2" => ProviderSource::Kind(ProviderKind::Oauth2),
            "oidc" => ProviderSource::Kind(ProviderKind::Oidc),
            other => {
                return Err(p.err_at(
                    k.span,
                    format!("unknown provider kind `{other}`: write `oauth2` or `oidc`"),
                ))
            }
        }
    };
    let mut label = None;
    while matches!(p.peek(), TokenKind::Comma) {
        p.bump();
        let opt = ident(p, "`label`")?;
        if opt.name != "label" {
            return Err(p.err_at(
                opt.span,
                format!("a provider takes `label \"…\"`, not `{}`", opt.name),
            ));
        }
        once(p, label.is_some(), &opt)?;
        match p.peek().clone() {
            TokenKind::Str(s) => {
                p.bump();
                label = Some(s);
            }
            other => {
                return Err(p.err_here(format!(
                    "`label` is the button text in quotes, `label \"Work account\"`; found {}",
                    describe(&other)
                )))
            }
        }
    }
    Ok(AuthProvider {
        name,
        source,
        label,
        span: here.to(p.prev_span()),
    })
}

/// `token <d> [, max <d>] [, tenants <d> to <d>]`.
fn session(p: &mut Parser, here: Span) -> Result<AuthSession, ParseError> {
    match p.peek() {
        TokenKind::Ident(w) if w == "token" => {
            p.bump();
        }
        other => {
            return Err(p.err_here(format!(
                "`session` begins with the token life, `session token 1h`; found {}",
                describe(other)
            )))
        }
    }
    let token = duration(p, "token")?;
    let mut max = None;
    let mut tenants = None;
    options(p, true, "session", &["max", "tenants"], |p, opt| {
        if opt.name == "max" {
            once(p, max.is_some(), opt)?;
            max = Some(duration(p, "max")?);
        } else {
            once(p, tenants.is_some(), opt)?;
            let min = duration(p, "tenants")?;
            match p.peek() {
                TokenKind::Ident(w) if w == "to" => {
                    p.bump();
                }
                other => {
                    return Err(p.err_here(format!(
                        "`tenants` is a range, `tenants 15m to 4h`; found {}",
                        describe(other)
                    )))
                }
            }
            let hi = duration(p, "tenants")?;
            tenants = Some(AuthRange {
                span: opt.span.to(hi.span),
                min,
                max: hi,
            });
        }
        Ok(())
    })?;
    Ok(AuthSession {
        token,
        max,
        tenants,
        span: here.to(p.prev_span()),
    })
}
