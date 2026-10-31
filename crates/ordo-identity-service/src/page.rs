//! The service's own page (`plan-identity-service.md` 9, step 2): plain
//! HTML and JavaScript, built into the binary, no build step.
//!
//! | Route | What |
//! |---|---|
//! | `GET /` | sends the browser to `/account` |
//! | `GET /join` | sign up by invite link: a name, a passkey, recovery codes shown once (5.3) |
//! | `GET /sign-in` | a passkey, or a recovery code that may only enrol a new passkey (5.4) |
//! | `GET /cli` | approve or deny `ordo login` on this computer, with a touch (5.5) |
//! | `GET /account` | passkeys (list, add, remove, never the last), new recovery codes |
//! | `GET /page/<file>` | the page's scripts and stylesheet |
//!
//! Every page is sent with a strict policy ([`PAGE_CSP`]): scripts and
//! styles from this origin only, no inline script, no other site, never
//! framed. Pages are `no-store`, since sign-up and new recovery codes show
//! secrets. The scripts call the JSON routes in [`crate::routes`] on the
//! same origin.

use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::server::Service;

/// The policy every page is sent with. `require-trusted-types-for` makes
/// the browser refuse any HTML written from a string, so the scripts build
/// the page from elements and text only.
pub const PAGE_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
     img-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; \
     base-uri 'none'; require-trusted-types-for 'script'; trusted-types 'none'";

/// Passkeys on this origin only; nothing else the page does not use.
const PERMISSIONS: &str = "publickey-credentials-create=(self), publickey-credentials-get=(self), \
     camera=(), microphone=(), geolocation=(), payment=()";

/// Where `/sign-in` writes the origins it may send a browser back to.
const RETURN_ORIGINS: &str = "__ORDO_RETURN_ORIGINS__";
/// Filled with the base path (`ORDO_IDENTITY_BASE_PATH`) the page's
/// scripts prefix every call and link with.
const BASE: &str = "__ORDO_BASE__";

/// The page's files: the name served under `/page/`, its media type, and
/// its text.
pub const ASSETS: &[(&str, &str, &str)] = &[
    (
        "identity.css",
        "text/css; charset=utf-8",
        include_str!("../page/identity.css"),
    ),
    (
        "common.js",
        "text/javascript; charset=utf-8",
        include_str!("../page/common.js"),
    ),
    (
        "webauthn.js",
        "text/javascript; charset=utf-8",
        include_str!("../page/webauthn.js"),
    ),
    (
        "join.js",
        "text/javascript; charset=utf-8",
        include_str!("../page/join.js"),
    ),
    (
        "sign-in.js",
        "text/javascript; charset=utf-8",
        include_str!("../page/sign-in.js"),
    ),
    (
        "cli.js",
        "text/javascript; charset=utf-8",
        include_str!("../page/cli.js"),
    ),
    (
        "account.js",
        "text/javascript; charset=utf-8",
        include_str!("../page/account.js"),
    ),
];

/// The pages: path and HTML.
pub const PAGES: &[(&str, &str)] = &[
    ("/join", include_str!("../page/join.html")),
    ("/sign-in", include_str!("../page/sign-in.html")),
    ("/cli", include_str!("../page/cli.html")),
    ("/account", include_str!("../page/account.html")),
];

pub(crate) fn routes() -> Router<Service> {
    let mut r = Router::new()
        .route("/", get(home))
        .route("/page/{file}", get(asset));
    for (path, _) in PAGES {
        r = r.route(path, get(page));
    }
    r
}

pub(crate) async fn home(State(s): State<Service>) -> Response {
    let to = format!("{}/account", s.config().base_path);
    (StatusCode::SEE_OTHER, [(LOCATION, to)]).into_response()
}

async fn page(State(s): State<Service>, uri: axum::http::Uri) -> Response {
    let Some((_, html)) = PAGES.iter().find(|(p, _)| *p == uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let base = s.config().base_path.as_str();
    let mut body = (*html).to_string();
    if !base.is_empty() {
        // Every link and file the page names is rooted: put it under the
        // base path, so it reaches the service and not what is beside it.
        // Before the fills below, whose values may start with `/`.
        body = body.replace("=\"/", &format!("=\"{base}/"));
    }
    body = body.replace(RETURN_ORIGINS, &attr(&return_origins(&s)));
    body = body.replace(BASE, &attr(base));
    let mut res = body.into_response();
    let h = res.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(PAGE_CSP));
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(PERMISSIONS),
    );
    res
}

async fn asset(Path(file): Path<String>) -> Response {
    let Some((_, kind, text)) = ASSETS.iter().find(|(name, _, _)| *name == file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut res = (*text).into_response();
    let h = res.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static(kind));
    // Checked again on every load: a new release's script is never mixed
    // with an old page.
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    res
}

/// The service's own origin and `ORDO_IDENTITY_CLIENT_ORIGINS`, space
/// separated: where sign-in may send a browser back to.
fn return_origins(s: &Service) -> String {
    let cfg = s.config();
    let mut out = vec![cfg.origin.trim_end_matches('/').to_string()];
    out.extend(cfg.client_origins.iter().map(|o| o.to_string()));
    out.join(" ")
}

/// Text made safe inside a double-quoted HTML attribute.
fn attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_loads_only_files_that_are_served_and_no_inline_script() {
        for (path, html) in PAGES {
            assert!(!html.contains("<script>"), "{path} has inline script");
            assert!(!has_handler(html), "{path} has an event handler attribute");
            assert!(!html.contains("style="), "{path} has an inline style");
            for piece in html.split("/page/").skip(1) {
                let name = piece.split('"').next().unwrap_or_default();
                assert!(
                    ASSETS.iter().any(|(n, _, _)| *n == name),
                    "{path} loads /page/{name}, which is not served"
                );
            }
        }
    }

    /// ` on<letters>=`: an inline event handler.
    fn has_handler(html: &str) -> bool {
        html.match_indices(" on").any(|(i, _)| {
            let rest = &html[i + 3..];
            let name = rest.chars().take_while(char::is_ascii_lowercase).count();
            name > 0 && rest[name..].starts_with('=')
        })
    }

    #[test]
    fn the_page_text_has_no_em_dash() {
        for (path, html) in PAGES {
            assert!(!html.contains('\u{2014}'), "{path} has an em dash");
        }
        for (name, _, text) in ASSETS {
            assert!(!text.contains('\u{2014}'), "{name} has an em dash");
        }
    }

    #[test]
    fn an_attribute_cannot_be_closed_by_the_text_put_in_it() {
        assert_eq!(attr(r#"a"b<c>&"#), "a&quot;b&lt;c&gt;&amp;");
    }
}
