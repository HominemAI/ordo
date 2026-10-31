//! The console page (`plan-console.md` 5.1, 5.4): the built React page,
//! served from a directory.
//!
//! The page is found at `ORDO_CONSOLE_PAGE_DIR` (default
//! `/usr/share/ordo-console/page`, where `docker/Dockerfile` puts it). A
//! directory rather than files embedded at build time, so `cargo build`
//! needs no Node toolchain and the page can be rebuilt without relinking
//! the server. The image is what pins the two together.
//!
//! `index.html` is read once at boot and answers every page route under
//! `/d/<handle>/`, so the page does its own routing. Hashed files under
//! `/assets/` are read from disk per request and may be cached for good:
//! their names change when their bytes do. A missing page is not a boot
//! refusal (sign-in and health still answer through the JSON API); the
//! page routes then answer with a short note that names the setting.

use std::path::{Path, PathBuf};

use axum::body::Bytes;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::config::PAGE_DIR_ENV;

/// The page as found at boot.
pub struct Page {
    dir: PathBuf,
    index: Option<Bytes>,
}

impl Page {
    /// Read `index.html` from `dir`. A missing or unreadable page is logged
    /// once, with the setting to change.
    pub fn load(dir: &Path) -> Page {
        let index = match std::fs::read(dir.join("index.html")) {
            Ok(b) => Some(Bytes::from(b)),
            Err(e) => {
                eprintln!(
                    "ordo-console: no page at {} ({e}). The JSON API and health still answer. \
                     Set {PAGE_DIR_ENV} to the folder that holds the built page's index.html.",
                    dir.display()
                );
                None
            }
        };
        Page {
            dir: dir.to_path_buf(),
            index,
        }
    }

    /// Put `text` at the top of every page, before the app's root, in the
    /// page's own `note` style. A `note`, never a live region: it is not
    /// announced on every page, and the page's own status line stays the
    /// only `status` there. Server-side, so the banner
    /// shows whatever the page's build, and it stays above every screen.
    /// A page with neither a `<body>` tag nor the root is served as it is.
    pub fn with_notice(mut self, text: &str) -> Page {
        let Some(bytes) = &self.index else {
            return self;
        };
        let Ok(html) = std::str::from_utf8(bytes) else {
            return self;
        };
        // After the `<body>` tag; a page that leaves it implicit, before
        // the app's root.
        let Some(at) = html
            .find("<body")
            .and_then(|i| html[i..].find('>').map(|j| i + j + 1))
            .or_else(|| html.find("<div id=\"root\""))
        else {
            return self;
        };
        let banner = format!(
            "<p class=\"note\" role=\"note\" aria-label=\"Sign-in notice\" id=\"ordo-console-notice\">{}</p>",
            escape(text)
        );
        let mut out = String::with_capacity(html.len() + banner.len());
        out.push_str(&html[..at]);
        out.push_str(&banner);
        out.push_str(&html[at..]);
        self.index = Some(Bytes::from(out));
        self
    }

    /// Whether the built page was found.
    pub fn found(&self) -> bool {
        self.index.is_some()
    }

    /// The page shell, for any page route.
    pub fn index(&self) -> Response {
        match &self.index {
            Some(b) => html(StatusCode::OK, b.clone()),
            None => html(
                StatusCode::SERVICE_UNAVAILABLE,
                Bytes::from(format!(
                    "<!doctype html><meta charset=\"utf-8\"><title>Ordo console</title>\
                     <p>The console is running, but its page was not found at {}. \
                     Set {PAGE_DIR_ENV} to the folder that holds the built page.</p>",
                    escape(&self.dir.display().to_string())
                )),
            ),
        }
    }

    /// One file under `assets/`.
    pub async fn asset(&self, rel: &str) -> Response {
        let Some(path) = safe_path(rel) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(kind) = content_type(&path) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        match tokio::fs::read(self.dir.join("assets").join(&path)).await {
            Ok(bytes) => {
                let mut res = (StatusCode::OK, bytes).into_response();
                let h = res.headers_mut();
                h.insert(CONTENT_TYPE, HeaderValue::from_static(kind));
                h.insert(
                    CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                );
                res
            }
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

fn html(status: StatusCode, body: Bytes) -> Response {
    let mut res = (status, body).into_response();
    res.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    res
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A relative path of plain segments: no `..`, no hidden files, no
/// backslashes, nothing but letters, digits, `.`, `_` and `-`.
fn safe_path(rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.len() > 200 {
        return None;
    }
    let mut out = PathBuf::new();
    for seg in rel.split('/') {
        let ok = !seg.is_empty()
            && !seg.starts_with('.')
            && seg
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
        if !ok {
            return None;
        }
        out.push(seg);
    }
    Some(out)
}

/// The types the page's build makes. Anything else is not served.
fn content_type(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "json" => "application/json",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_paths_are_served() {
        assert!(safe_path("index-abc.js").is_some());
        assert!(safe_path("fonts/a.woff2").is_some());
        for bad in [
            "",
            "../x.js",
            "a/../../x.js",
            ".env",
            "a//b.js",
            "a\\b.js",
            "%2e%2e/x",
        ] {
            assert!(safe_path(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_notice_goes_first_in_the_body_escaped() {
        let page = Page {
            dir: PathBuf::from("/x"),
            index: Some(Bytes::from_static(
                b"<html><body class=\"a\"><div id=\"root\"></div></body></html>",
            )),
        }
        .with_notice("set A=<b> & go");
        let html = String::from_utf8(page.index.unwrap().to_vec()).unwrap();
        assert_eq!(
            html,
            "<html><body class=\"a\"><p class=\"note\" role=\"note\" aria-label=\"Sign-in notice\" \
             id=\"ordo-console-notice\">set A=&lt;b&gt; &amp; go</p><div id=\"root\"></div>\
             </body></html>"
        );
        let none = Page {
            dir: PathBuf::from("/x"),
            index: None,
        }
        .with_notice("x");
        assert!(!none.found());
    }

    #[test]
    fn only_known_types_are_served() {
        assert_eq!(
            content_type(Path::new("a.js")),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(content_type(Path::new("a.map")), None);
        assert_eq!(content_type(Path::new("a")), None);
    }
}
