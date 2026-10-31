//! 14g: the CORS allowlist at the door.
//!
//! # What this answers
//!
//! A browser page on another origin can call a deployment only if the
//! deployment says so. Every Ordo call carries `Authorization` and
//! `X-Ordo-Space`, so every browser call is preflighted: the browser sends
//! `OPTIONS` first and refuses to send the real request unless the answer
//! names the origin. Without this layer the preflight answered 405 and the
//! real answer carried no `Access-Control-Allow-Origin`, so the browser
//! discarded a good response in silence.
//!
//! # One layer, over the whole door
//!
//! [`apply`] wraps a finished router. The preflight is answered before
//! routing, so it does not depend on which route the path names, and the
//! actual responses of the op, blob and platform routes all pass through
//! the same code.
//!
//! # The list, and why it has no wildcard
//!
//! [`CorsPolicy::parse`] takes a comma-separated list of exact origins, such
//! as `https://app.example.com,http://localhost:5173`. Each entry is a
//! scheme, a host and an optional port, with no path.
//!
//! `*` is refused. Every Ordo call carries a bearer token in `Authorization`,
//! which is a credential. A wildcard would let a script on any site in the
//! world make the call from a visitor's browser the moment it holds a token,
//! and the operator would have chosen that by accident of convenience. A
//! deployment that really wants every origin can list them, and a list is
//! what makes the choice visible in review.
//!
//! # Credentials, on `/auth/` only
//!
//! The door reads one cookie: the renewal cookie that keeps a person signed
//! in across a reload, sent only to `/auth/refresh`. A page on another
//! origin can store and send it only when the answer says
//! `Access-Control-Allow-Credentials: true`. The door says so on `/auth/`
//! paths, and only beside an exact origin a list names, never beside `*`.
//! The op and blob routes never say it: they read no cookie.
//!
//! # One label, when switched on
//!
//! `https://*.example.com` is accepted only when `ORDO_WILDCARD_ORIGINS=on`
//! (owner ruling 2026-10-09, `subdomain-signin` ask 3), and is otherwise
//! refused by name. It matches one label (`https://app.example.com`), never
//! the bare domain, never two labels, https only. The bare `*` stays refused
//! whatever the setting says.
//!
//! This is safe where `*` is not, for two reasons. The answer still names
//! the one origin that asked, never `*`, with `Vary: Origin`. And the op and
//! blob routes read no cookie, so a page on a matching origin gains nothing
//! there that it did not already hold: it must bring a bearer token of its
//! own. **The renewal cookie is the exception, on `/auth/refresh` only.**
//! With `ORDO_WILDCARD_ORIGINS=on`, a page on any matching subdomain may send
//! the renewal cookie there with credentials and read back a fresh token, so
//! every subdomain the wildcard matches can redeem a person's renewal. What
//! the wildcard widens is therefore which pages may read an answer to a
//! token they hold, and which may redeem a renewal; every page under the
//! domain is the app developer's to serve, and a subdomain that is not should
//! not be matched.
//!
//! Absent or empty means no policy: the door behaves as before, except for
//! public answers below.
//!
//! # A tenant's own list
//!
//! A tenant may list origins of its own, when the deployment allows it
//! (`ORDO_CORS_TENANT_ORIGINS_MAX`, read by the host; [`TenantOrigins`]).
//! The effective list for a space is the deployment's plus that tenant's
//! (`design-public-path.md` §8). A preflight cannot know which space the real
//! request is for, so it answers the union of every list. That tells an
//! origin only about itself. The real response answers the exact list for
//! the space in `X-Ordo-Space`, and that is the enforcement: an origin on
//! one tenant's list cannot read another tenant's answers.
//!
//! # Public answers
//!
//! An answer of the public path is made with no credential and holds only
//! public data, so it carries `Access-Control-Allow-Origin: *` whatever the
//! lists say. The router marks it with [`PublicAnswer`]. A preflight from an
//! origin on no list is answered `*` only when it is for a **public** op
//! ([`PublicOps`]) and does not ask to send `Authorization`, allowing
//! `X-Ordo-Human` and `Idempotency-Key` for a public write. Every other
//! preflight is checked against the lists, so a page on no list learns
//! nothing about a members' op from its preflight.
//!
//! # Hardening headers
//!
//! Every response through the layer carries `X-Content-Type-Options:
//! nosniff`, `X-Frame-Options: DENY` and `Referrer-Policy: no-referrer`,
//! unless the route set its own ([`harden`]).
//!
//! # What a refusal looks like
//!
//! A preflight from an origin not on the list answers 403 with no
//! `Access-Control-Allow-*` headers, which the browser reports as a CORS
//! failure. An actual request from such an origin is still served, because
//! CORS is a browser rule and a non-browser client sends no `Origin`; the
//! response simply lacks `Access-Control-Allow-Origin`, so the page cannot
//! read it.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::header::{HeaderName, HeaderValue, ORIGIN, VARY};
use axum::http::{Method, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::Response;
use axum::Router;

/// Methods the door serves.
const ALLOW_METHODS: &str = "GET, POST, PUT, DELETE, OPTIONS";

/// Request headers an Ordo call may carry.
const ALLOW_HEADERS: &str = "Authorization, Content-Type, Idempotency-Key, X-Ordo-Space, \
     X-Ordo-Page, X-Ordo-Resume, X-Ordo-Version, X-Ordo-Human, Last-Event-ID";

/// Response headers a page is allowed to read.
const EXPOSE_HEADERS: &str = "X-Ordo-Op-Version, X-Ordo-Version-Skew, X-Ordo-Idempotent-Replay, \
     X-Ordo-Page, X-Ordo-Position, X-Ordo-Head, X-Ordo-Head-Key, X-Ordo-Head-Sig, \
     X-Ordo-Verifiable-From, X-Ordo-Cache, ETag, Retry-After, Allow, Content-Disposition";

/// The methods and headers a public call's preflight allows. No
/// `Authorization`: a page that asks to send one is not making a public call.
const PUBLIC_METHODS: &str = "GET, POST, OPTIONS";
const PUBLIC_HEADERS: &str = "Content-Type, Idempotency-Key, X-Ordo-Page, X-Ordo-Version, \
     X-Ordo-Human";

/// Set by the router on every answer of the public path, success or
/// refusal. The layer answers such a response with
/// `Access-Control-Allow-Origin: *` (`design-public-path.md` §8, "Public
/// requests"): it was made with no credential, so no page can spend one
/// through it, and the data is public.
#[derive(Clone, Copy, Debug)]
pub struct PublicAnswer;

/// The tenants' own origin lists (`design-public-path.md` §8, "The layers").
/// The host keeps them in memory; the layer asks per request.
pub trait TenantOrigins: Send + Sync {
    /// Whether `space`'s own list names `origin`.
    fn allows(&self, space: &str, origin: &str) -> bool;
    /// Whether any tenant's list names `origin`. A preflight cannot know the
    /// space the real request is for, so it answers the union.
    fn any_allows(&self, origin: &str) -> bool;
}

/// Which ops answer a visitor (`design-public-path.md` §1). Asked by a
/// preflight from an origin on no list: only a public op's is answered `*`.
pub trait PublicOps: Send + Sync {
    fn is_public(&self, op: &str) -> bool;
}

impl PublicOps for crate::registry::RegistryHandle {
    fn is_public(&self, op: &str) -> bool {
        self.current()
            .get(op)
            .is_some_and(|o| o.public != crate::spec::PublicMode::None)
    }
}

/// What the layer holds: the deployment's list, the tenants' lists, and
/// which ops are public. With no [`PublicOps`], no op is.
#[derive(Clone, Default)]
pub struct Cors {
    pub deployment: Option<CorsPolicy>,
    pub tenants: Option<Arc<dyn TenantOrigins>>,
    pub public_ops: Option<Arc<dyn PublicOps>>,
}

/// How long a browser may cache a preflight answer, in seconds.
const MAX_AGE_SECS: &str = "600";

/// An allowlist of exact origins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorsPolicy {
    origins: Vec<String>,
    /// `https://*.<domain>` entries, present only when wildcards are on.
    wildcards: Vec<ordo_identity::origin::OriginPattern>,
}

impl CorsPolicy {
    /// Parse a comma-separated list. `Ok(None)` for an empty list, which
    /// means same-origin only. An entry that is not an exact origin is an
    /// error naming the entry.
    pub fn parse(list: &str) -> Result<Option<CorsPolicy>, String> {
        CorsPolicy::parse_with(list, false)
    }

    /// [`CorsPolicy::parse`], with `ORDO_WILDCARD_ORIGINS` read as
    /// `wildcards`. See the module doc for what a wildcard entry matches.
    pub fn parse_with(list: &str, wildcards: bool) -> Result<Option<CorsPolicy>, String> {
        let mut origins = Vec::new();
        let mut patterns = Vec::new();
        for raw in list.split(',') {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            if entry == "*" {
                return Err("`*` is refused: every call carries a bearer token, so a \
                     wildcard would let a script on any site use one. List the origins \
                     instead"
                    .into());
            }
            if entry.contains('*') {
                patterns.push(ordo_identity::origin::OriginPattern::parse(
                    entry, false, wildcards,
                )?);
                continue;
            }
            origins.push(validate(entry)?);
        }
        origins.sort();
        origins.dedup();
        Ok(
            (!origins.is_empty() || !patterns.is_empty()).then_some(CorsPolicy {
                origins,
                wildcards: patterns,
            }),
        )
    }

    /// The origins on the list, lowercased and sorted.
    pub fn origins(&self) -> &[String] {
        &self.origins
    }

    /// Whether `origin` is on the list, exactly or by an allowed wildcard.
    /// The host asks this for the one cookie the door reads.
    pub fn allows_origin(&self, origin: &str) -> bool {
        self.allows(origin)
    }

    fn allows(&self, origin: &str) -> bool {
        let origin = origin.to_ascii_lowercase();
        self.origins.contains(&origin)
            || (!self.wildcards.is_empty()
                && ordo_identity::origin::Origin::parse(&origin)
                    .is_some_and(|o| self.wildcards.iter().any(|p| p.matches(&o))))
    }
}

fn validate(entry: &str) -> Result<String, String> {
    let lower = entry.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return Err(format!(
            "`{entry}` is not an origin: it must start with http:// or https://"
        ));
    };
    let bad = rest.is_empty()
        || rest
            .contains(|c: char| c == '/' || c == '?' || c == '#' || c == '@' || c.is_whitespace())
        || rest.contains('*');
    if bad {
        return Err(format!(
            "`{entry}` is not an origin: give scheme, host and optional port only, \
             with no path, no trailing slash and no wildcard"
        ));
    }
    if let Some((host, port)) = rest.rsplit_once(':') {
        if !rest.ends_with(']') && (host.is_empty() || port.parse::<u16>().is_err()) {
            return Err(format!("`{entry}` has an invalid port"));
        }
    }
    Ok(lower)
}

/// Wrap `router` in the deployment's list alone. See [`apply_layers`].
pub fn apply(router: Router, policy: Option<CorsPolicy>) -> Router {
    apply_layers(
        router,
        Cors {
            deployment: policy,
            ..Cors::default()
        },
    )
}

/// Wrap `router` in the CORS layer. Always installed, because a public
/// answer is readable from any page whatever the lists say. With no list
/// and no tenant lists, the layer touches public answers and public
/// preflights only, and every other response is as it was.
pub fn apply_layers(router: Router, cors: Cors) -> Router {
    router
        .layer(from_fn_with_state(Arc::new(cors), cors_layer))
        .layer(axum::middleware::map_response(harden))
}

/// The hardening headers every door response carries, preflights and
/// refusals included. A route that set its own (a file served under its own
/// policy) keeps it.
pub async fn harden(mut response: Response) -> Response {
    let h = response.headers_mut();
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
    ] {
        h.entry(name).or_insert(HeaderValue::from_static(value));
    }
    response
}

fn vary_origin(response: &mut Response) {
    response
        .headers_mut()
        .append(VARY, HeaderValue::from_static("Origin"));
}

/// A preflight for a public call: to an op, asking to send no
/// `Authorization`. Answered `*` whatever the lists say. A page can already
/// send a tokenless POST as a form, and the door answers a tokenless call
/// to anything that is not public with its one uniform denial, so letting
/// any page read that answer discloses nothing.
fn public_preflight(request: &Request, public: Option<&dyn PublicOps>) -> bool {
    let Some(op) = request
        .uri()
        .path()
        .strip_prefix("/op/")
        .map(|rest| rest.split('/').next().unwrap_or(rest))
        .filter(|op| !op.is_empty())
    else {
        return false;
    };
    if !public.is_some_and(|p| p.is_public(op)) {
        return false;
    }
    let asks_for_credential = request
        .headers()
        .get("access-control-request-headers")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|h| {
            h.split(',')
                .any(|n| n.trim().eq_ignore_ascii_case("authorization"))
        });
    !asks_for_credential
}

/// A sign-in route, where the renewal cookie is set, sent or cleared.
fn auth_path(request: &Request) -> bool {
    request.uri().path().starts_with("/auth/")
}

async fn cors_layer(State(cors): State<Arc<Cors>>, request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let installed = cors.deployment.is_some() || cors.tenants.is_some();
    let Some(origin) = origin else {
        let mut response = next.run(request).await;
        if installed {
            vary_origin(&mut response);
        }
        return response;
    };
    let by_deployment = cors.deployment.as_ref().is_some_and(|p| p.allows(&origin));
    let preflight = request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(HeaderName::from_static("access-control-request-method"));

    if preflight {
        let allowed = by_deployment
            || cors
                .tenants
                .as_ref()
                .is_some_and(|t| t.any_allows(&origin.to_ascii_lowercase()));
        let mut response = Response::new(axum::body::Body::empty());
        if !allowed && public_preflight(&request, cors.public_ops.as_deref()) {
            *response.status_mut() = StatusCode::NO_CONTENT;
            let h = response.headers_mut();
            h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
            h.insert(
                "access-control-allow-methods",
                HeaderValue::from_static(PUBLIC_METHODS),
            );
            h.insert(
                "access-control-allow-headers",
                HeaderValue::from_static(PUBLIC_HEADERS),
            );
            h.insert(
                "access-control-max-age",
                HeaderValue::from_static(MAX_AGE_SECS),
            );
            return response;
        }
        if !installed {
            // No list at all: the door answers as it did before the layer.
            return next.run(request).await;
        }
        if allowed {
            let credentials = auth_path(&request);
            *response.status_mut() = StatusCode::NO_CONTENT;
            let h = response.headers_mut();
            if let Ok(v) = HeaderValue::from_str(&origin) {
                h.insert("access-control-allow-origin", v);
                if credentials {
                    h.insert(
                        "access-control-allow-credentials",
                        HeaderValue::from_static("true"),
                    );
                }
            }
            h.insert(
                "access-control-allow-methods",
                HeaderValue::from_static(ALLOW_METHODS),
            );
            h.insert(
                "access-control-allow-headers",
                HeaderValue::from_static(ALLOW_HEADERS),
            );
            h.insert(
                "access-control-max-age",
                HeaderValue::from_static(MAX_AGE_SECS),
            );
        } else {
            *response.status_mut() = StatusCode::FORBIDDEN;
        }
        response.headers_mut().append(
            VARY,
            HeaderValue::from_static(
                "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
            ),
        );
        return response;
    }

    // The tenant layer answers for the space the request names. A request
    // that names it only inside a POST body is answered by the deployment's
    // list alone; the SDK always sends the header.
    let space = request
        .headers()
        .get("x-ordo-space")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string());
    let credentials = auth_path(&request);
    let mut response = next.run(request).await;
    if response.extensions().get::<PublicAnswer>().is_some() {
        let h = response.headers_mut();
        h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
        h.insert(
            "access-control-expose-headers",
            HeaderValue::from_static(EXPOSE_HEADERS),
        );
        return response;
    }
    let allowed = by_deployment
        || match (&cors.tenants, &space) {
            (Some(t), Some(space)) => t.allows(space, &origin.to_ascii_lowercase()),
            _ => false,
        };
    if allowed {
        if let Ok(v) = HeaderValue::from_str(&origin) {
            let h = response.headers_mut();
            h.insert("access-control-allow-origin", v);
            if credentials {
                h.insert(
                    "access-control-allow-credentials",
                    HeaderValue::from_static("true"),
                );
            }
            h.insert(
                "access-control-expose-headers",
                HeaderValue::from_static(EXPOSE_HEADERS),
            );
        }
    }
    if installed {
        vary_origin(&mut response);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_or_empty_is_no_policy() {
        assert_eq!(CorsPolicy::parse("").unwrap(), None);
        assert_eq!(CorsPolicy::parse(" , ").unwrap(), None);
    }

    #[test]
    fn a_list_is_normalised() {
        let p = CorsPolicy::parse(
            "HTTPS://App.Example.com, http://localhost:5173,https://app.example.com",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            p.origins(),
            ["http://localhost:5173", "https://app.example.com"]
        );
        assert!(p.allows("https://APP.example.com"));
        assert!(!p.allows("https://app.example.com:444"));
    }

    #[test]
    fn local_subdomains_are_listed_and_matched_exactly() {
        let p = CorsPolicy::parse("http://shop.localhost:5181,http://login.localhost:5180")
            .unwrap()
            .unwrap();
        assert!(p.allows("http://shop.localhost:5181"));
        assert!(!p.allows("http://shop.localhost:5182"));
        assert!(!p.allows("http://admin.localhost:5181"));
    }

    #[test]
    fn a_wildcard_is_refused() {
        assert!(CorsPolicy::parse("*").unwrap_err().contains("refused"));
        assert!(CorsPolicy::parse("https://a.example,*").is_err());
        let off = CorsPolicy::parse("https://*.example.com").unwrap_err();
        assert!(off.contains("ORDO_WILDCARD_ORIGINS=on"), "{off}");
        assert!(CorsPolicy::parse_with("*", true).is_err());
    }

    #[test]
    fn a_wildcard_when_switched_on_matches_one_label_only() {
        let p = CorsPolicy::parse_with("https://*.example.com", true)
            .unwrap()
            .unwrap();
        assert!(p.allows("https://app.example.com"));
        assert!(p.allows("https://X.example.com"));
        assert!(!p.allows("https://example.com"));
        assert!(!p.allows("https://a.b.example.com"));
        assert!(!p.allows("https://x.example.com.evil.com"));
        assert!(!p.allows("https://evil.com"));
        assert!(!p.allows("http://x.example.com"));
        assert!(CorsPolicy::parse_with("http://*.example.com", true).is_err());
        assert!(CorsPolicy::parse_with("https://*.com", true).is_err());
    }

    #[test]
    fn a_non_origin_is_refused() {
        for bad in [
            "app.example.com",
            "https://app.example.com/",
            "https://app.example.com/x",
            "null",
            "ftp://x",
            "https://",
            "http://host:notaport",
        ] {
            assert!(CorsPolicy::parse(bad).is_err(), "{bad}");
        }
    }
}
