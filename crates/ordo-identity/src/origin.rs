//! Origins, and lists of them that may hold a wildcard.
//!
//! Three settings name the pages a deployment trusts:
//! `ORDO_AUTH_RETURN_URLS` (where a sign-in may send the browser back to),
//! `ORDO_AUTH_ORIGINS` (which page may run a passkey ceremony) and
//! `ORDO_CORS_ORIGINS` (which page may call the door). Each is compared
//! here, by origin: scheme, host and port (owner ruling 2026-10-09,
//! `subdomain-signin` ask 2).
//!
//! # Wildcards
//!
//! An entry such as `https://*.example.com` matches any **one** label in
//! place of the `*`: `https://app.example.com` and `https://x.example.com`,
//! and never `https://example.com`, `https://a.b.example.com` or
//! `https://x.example.com.evil.com`. It is https only, the `*` must be the
//! whole first label, and at least two labels must follow it, so `*.com`
//! is refused. Wildcards are off by default and an entry with one refuses
//! the boot until [`WILDCARD_SETTING`] is `on` (owner ruling 2026-10-09,
//! `subdomain-signin` ask 3).
//!
//! # What a URL's origin is, here
//!
//! [`Origin::of_url`] is deliberately narrow. It accepts what a page sends
//! and refuses anything a browser and this parser could read differently:
//! a user-info part (`https://app.example.com@evil.com`), a backslash, a
//! percent sign or any non-ASCII byte in the host, whitespace and control
//! characters anywhere, and an empty label. A refused URL is a refused
//! return, never a guess.

/// The setting that switches wildcard origins on, and the value that does.
pub const WILDCARD_SETTING: &str = "ORDO_WILDCARD_ORIGINS";

/// A scheme, a host and a port, lowercased, with the scheme's default port
/// dropped. Two of these are equal exactly when a browser would call them
/// the same origin.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin {
    scheme: String,
    host: String,
    port: Option<u16>,
}

impl Origin {
    /// The origin of a full URL: `scheme://host[:port]` followed by
    /// nothing, or by a path, a query or a fragment. `None` for anything
    /// that is not plainly that.
    pub fn of_url(url: &str) -> Option<Origin> {
        if url.is_empty()
            || !url.is_ascii()
            || url
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b == b'\\')
        {
            return None;
        }
        let (scheme, rest) = url.split_once("://")?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return None;
        }
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..end];
        Origin::of_authority(&scheme, authority)
    }

    /// An origin written as an origin: `scheme://host[:port]` and nothing
    /// after it. A trailing slash is refused, as a browser never sends one.
    pub fn parse(text: &str) -> Option<Origin> {
        let (_, rest) = text.split_once("://")?;
        if rest.contains(['/', '?', '#']) {
            return None;
        }
        Origin::of_url(text)
    }

    fn of_authority(scheme: &str, authority: &str) -> Option<Origin> {
        if authority.is_empty() || authority.contains('@') {
            return None;
        }
        let authority = authority.to_ascii_lowercase();
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (inside, after) = v6.split_once(']')?;
            if inside.is_empty() || !inside.bytes().all(|b| b.is_ascii_hexdigit() || b == b':') {
                return None;
            }
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':')?),
            };
            (format!("[{inside}]"), port)
        } else {
            let (host, port) = match authority.split_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (authority.clone(), None),
            };
            if !valid_host(&host) {
                return None;
            }
            (host, port)
        };
        let port = match port {
            None => None,
            Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => {
                Some(p.parse::<u16>().ok()?)
            }
            Some(_) => return None,
        };
        let default = match scheme {
            "https" => 443,
            _ => 80,
        };
        Some(Origin {
            scheme: scheme.to_string(),
            host,
            port: port.filter(|p| *p != default),
        })
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// A loopback host: what a developer's own machine serves a page from.
    pub fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }
}

/// A host a developer's own machine serves a page from: `localhost`,
/// `127.0.0.1`, `[::1]`, or exactly one label under `localhost`
/// (`shop.localhost`). Browsers treat all of these as secure contexts.
/// `a.b.localhost` and a host that merely ends in `localhost` are not.
/// The identity service's base path (`ORDO_IDENTITY_BASE_PATH`): where
/// browsers reach it under another origin, such as `/identity` under the
/// console's (step 7). Empty is the root. Otherwise `/`, then one or more
/// segments of lower-case letters, digits, `-` and `_`, no trailing `/`.
/// `Err` says what is wrong.
pub fn base_path(text: &str) -> Result<String, &'static str> {
    let t = text.trim().trim_end_matches('/');
    if t.is_empty() {
        return Ok(String::new());
    }
    if !t.starts_with('/') {
        return Err("it does not start with /");
    }
    if t.len() > 64 {
        return Err("it is longer than 64 characters");
    }
    let ok = t[1..].split('/').all(|seg| {
        !seg.is_empty()
            && seg
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    });
    if !ok {
        return Err("a segment is empty or holds a character other than a-z, 0-9, - and _");
    }
    Ok(t.to_string())
}

pub fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]") || is_localhost_subdomain(host)
}

/// `<label>.localhost`: one valid lowercase label, nothing more.
pub fn is_localhost_subdomain(host: &str) -> bool {
    host.strip_suffix(".localhost").is_some_and(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.port {
            Some(p) => write!(f, "{}://{}:{p}", self.scheme, self.host),
            None => write!(f, "{}://{}", self.scheme, self.host),
        }
    }
}

/// A host name of lowercase ASCII labels, or a dotted IPv4 address. No
/// empty label, so no trailing dot.
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// One entry of a list: an exact origin, or `https://*.<suffix>[:port]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginPattern {
    Exact(Origin),
    /// Any one label, then `.suffix`, on https, with this port.
    OneLabelUnder {
        suffix: String,
        port: Option<u16>,
    },
}

impl OriginPattern {
    /// Read one entry. `with_path` lets an entry carry a path, a query or
    /// nothing else after its origin, which is how `ORDO_AUTH_RETURN_URLS`
    /// was written before it matched by origin; the path is not kept.
    /// `wildcards` is [`WILDCARD_SETTING`]; an entry with a `*` when it is
    /// off is refused with text that names the setting and the value.
    pub fn parse(entry: &str, with_path: bool, wildcards: bool) -> Result<OriginPattern, String> {
        let entry = entry.trim();
        if entry.contains('*') {
            return OriginPattern::parse_wildcard(entry, with_path, wildcards);
        }
        let origin = if with_path {
            if entry.contains('#') {
                return Err(format!(
                    "`{entry}` has a fragment; give the address without it"
                ));
            }
            Origin::of_url(entry)
        } else {
            Origin::parse(entry)
        };
        origin.map(OriginPattern::Exact).ok_or_else(|| {
            if with_path {
                format!("`{entry}` is not an address: give https://host[:port] and any path")
            } else {
                format!(
                    "`{entry}` is not an origin: give scheme, host and optional port only, \
                     with no path and no trailing slash"
                )
            }
        })
    }

    fn parse_wildcard(
        entry: &str,
        with_path: bool,
        wildcards: bool,
    ) -> Result<OriginPattern, String> {
        if !wildcards {
            return Err(format!(
                "`{entry}` is a wildcard, and wildcards are off. Set {WILDCARD_SETTING}=on \
                 to allow it, or list each origin"
            ));
        }
        let bad = || {
            format!(
                "`{entry}` is not a wildcard origin: write https://*.example.com, with `*` \
                 as the whole first label and at least two labels after it"
            )
        };
        let rest = entry
            .strip_prefix("https://*.")
            .or_else(|| entry.strip_prefix("HTTPS://*."))
            .ok_or_else(bad)?;
        // Stand a real label in for the `*` and read it as any origin is
        // read, so a wildcard entry obeys every rule an exact one does.
        let probe = format!("https://wildcard-probe.{rest}");
        let origin = if with_path {
            Origin::of_url(&probe).filter(|_| !rest.contains('#'))
        } else {
            Origin::parse(&probe)
        }
        .ok_or_else(bad)?;
        let suffix = origin
            .host
            .strip_prefix("wildcard-probe.")
            .ok_or_else(bad)?
            .to_string();
        if !suffix.contains('.') || suffix.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return Err(bad());
        }
        Ok(OriginPattern::OneLabelUnder {
            suffix,
            port: origin.port,
        })
    }

    pub fn is_wildcard(&self) -> bool {
        matches!(self, OriginPattern::OneLabelUnder { .. })
    }

    pub fn matches(&self, origin: &Origin) -> bool {
        match self {
            OriginPattern::Exact(o) => o == origin,
            OriginPattern::OneLabelUnder { suffix, port } => {
                origin.scheme == "https"
                    && origin.port == *port
                    && origin
                        .host
                        .strip_suffix(suffix.as_str())
                        .and_then(|h| h.strip_suffix('.'))
                        .is_some_and(|label| !label.is_empty() && !label.contains('.'))
            }
        }
    }
}

impl std::fmt::Display for OriginPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OriginPattern::Exact(o) => write!(f, "{o}"),
            OriginPattern::OneLabelUnder { suffix, port } => match port {
                Some(p) => write!(f, "https://*.{suffix}:{p}"),
                None => write!(f, "https://*.{suffix}"),
            },
        }
    }
}

/// A comma-separated list of [`OriginPattern`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OriginList {
    entries: Vec<OriginPattern>,
}

impl OriginList {
    /// Read a list. Empty entries are skipped. See [`OriginPattern::parse`].
    pub fn parse(text: &str, with_path: bool, wildcards: bool) -> Result<OriginList, String> {
        let mut entries = Vec::new();
        for raw in text.split(',') {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            let p = OriginPattern::parse(entry, with_path, wildcards)?;
            if !entries.contains(&p) {
                entries.push(p);
            }
        }
        Ok(OriginList { entries })
    }

    pub fn from_patterns(entries: Vec<OriginPattern>) -> OriginList {
        OriginList { entries }
    }

    pub fn entries(&self) -> &[OriginPattern] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Does any entry match this origin?
    pub fn allows(&self, origin: &Origin) -> bool {
        self.entries.iter().any(|p| p.matches(origin))
    }

    /// Does any entry match the origin of this URL? A URL that does not
    /// read plainly ([`Origin::of_url`]) is refused.
    pub fn allows_url(&self, url: &str) -> bool {
        Origin::of_url(url).is_some_and(|o| self.allows(&o))
    }

    /// Does any entry match this origin, written as a browser sends it in
    /// an `Origin` header?
    pub fn allows_origin(&self, origin: &str) -> bool {
        Origin::parse(origin).is_some_and(|o| self.allows(&o))
    }
}

/// Read [`WILDCARD_SETTING`]'s value: `on` or `off`, unset is `off`.
pub fn wildcards_setting(value: Option<&str>) -> Result<bool, String> {
    match value.map(str::trim) {
        None | Some("") | Some("off") => Ok(false),
        Some("on") => Ok(true),
        Some(other) => Err(format!(
            "`{other}` is not a setting (on | off); unset it for the default, off"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_path_is_a_rooted_path_of_plain_segments() {
        assert_eq!(base_path("").unwrap(), "");
        assert_eq!(base_path("/").unwrap(), "");
        assert_eq!(base_path("/identity/").unwrap(), "/identity");
        assert_eq!(base_path("/a/sign-in_2").unwrap(), "/a/sign-in_2");
        for bad in ["identity", "/Identity", "/a//b", "/a b", "/../x", "/a?x"] {
            assert!(base_path(bad).is_err(), "{bad}");
        }
    }

    fn list(text: &str, wild: bool) -> OriginList {
        OriginList::parse(text, true, wild).expect("parses")
    }

    #[test]
    fn an_address_matches_by_origin_with_any_path() {
        let l = list("https://app.example.com/signed-in", false);
        assert!(l.allows_url("https://app.example.com/signed-in"));
        assert!(l.allows_url("https://app.example.com/somewhere/else?x=1"));
        assert!(l.allows_url("https://APP.example.com"));
        assert!(l.allows_url("https://app.example.com:443/x"));
        assert!(!l.allows_url("https://app.example.com:8443/x"));
        assert!(!l.allows_url("http://app.example.com/x"));
        assert!(!l.allows_url("https://other.example.com/x"));
    }

    #[test]
    fn urls_that_read_two_ways_are_refused() {
        let l = list("https://app.example.com", false);
        for bad in [
            "https://app.example.com@evil.com/",
            "https://evil.com\\@app.example.com/",
            "https://app.example.com.evil.com/",
            "https://app.example.com./",
            "https://app%2eexample.com/",
            "https://app.example.com /",
            "javascript://app.example.com/",
            "//app.example.com/",
            "https://app.example.com:/",
            "https://app.example.com:99999/",
        ] {
            assert!(!l.allows_url(bad), "{bad}");
        }
    }

    #[test]
    fn a_wildcard_is_refused_while_the_setting_is_off_and_names_it() {
        let e = OriginList::parse("https://*.example.com", true, false).unwrap_err();
        assert!(e.contains("ORDO_WILDCARD_ORIGINS=on"), "{e}");
    }

    #[test]
    fn a_wildcard_matches_one_label_and_nothing_else() {
        let l = list("https://*.example.com", true);
        assert!(l.allows_url("https://x.example.com/any/path"));
        assert!(l.allows_url("https://app.example.com"));
        assert!(!l.allows_url("https://example.com"));
        assert!(!l.allows_url("https://a.b.example.com"));
        assert!(!l.allows_url("https://x.example.com.evil.com"));
        assert!(!l.allows_url("https://evil.com"));
        assert!(!l.allows_url("https://xexample.com"));
        assert!(!l.allows_url("http://x.example.com"));
        assert!(!l.allows_url("https://x.example.com:8443"));
    }

    #[test]
    fn a_wildcard_must_be_https_one_whole_label_and_under_two_labels() {
        for bad in [
            "http://*.example.com",
            "https://*.com",
            "https://*",
            "https://a*.example.com",
            "https://x.*.example.com",
            "https://*.*.example.com",
            "https://*.1.2.3",
        ] {
            assert!(OriginList::parse(bad, true, true).is_err(), "{bad}");
        }
        let with_port = list("https://*.example.com:8443", true);
        assert!(with_port.allows_url("https://x.example.com:8443/"));
        assert!(!with_port.allows_url("https://x.example.com/"));
    }

    #[test]
    fn an_origin_list_refuses_a_path() {
        assert!(OriginList::parse("https://a.example.com/x", false, false).is_err());
        assert!(OriginList::parse("https://a.example.com/", false, false).is_err());
        let l = OriginList::parse("https://a.example.com, http://localhost:5173", false, false)
            .expect("parses");
        assert!(l.allows_origin("https://a.example.com"));
        assert!(l.allows_origin("http://localhost:5173"));
        assert!(!l.allows_origin("https://a.example.com/"));
    }

    #[test]
    fn one_label_under_localhost_is_loopback_and_nothing_broader() {
        for yes in [
            "localhost",
            "127.0.0.1",
            "[::1]",
            "shop.localhost",
            "a-1.localhost",
        ] {
            assert!(is_loopback_host(yes), "{yes}");
        }
        for no in [
            "a.b.localhost",
            ".localhost",
            "-x.localhost",
            "shop.localhost.evil.com",
            "evillocalhost",
            "x.notlocalhost",
            "Shop.localhost",
            "example.com",
        ] {
            assert!(!is_loopback_host(no), "{no}");
        }
        let o = Origin::parse("http://shop.localhost:5181").expect("origin");
        assert!(o.is_loopback());
        assert!(!Origin::parse("http://a.b.localhost")
            .expect("origin")
            .is_loopback());
        let l = OriginList::parse(
            "http://shop.localhost:5181, http://login.localhost:5180",
            false,
            false,
        )
        .expect("parses");
        assert!(l.allows_origin("http://shop.localhost:5181"));
        assert!(!l.allows_origin("http://shop.localhost:5182"));
        assert!(!l.allows_origin("http://evil.localhost:5181"));
    }

    #[test]
    fn the_setting_reads_on_and_off_only() {
        assert_eq!(wildcards_setting(None), Ok(false));
        assert_eq!(wildcards_setting(Some("on")), Ok(true));
        assert_eq!(wildcards_setting(Some("off")), Ok(false));
        assert!(wildcards_setting(Some("yes")).is_err());
    }
}
