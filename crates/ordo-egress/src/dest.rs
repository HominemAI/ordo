//! Pins 1, 2 and 6: what a destination is, and how one becomes dialable.
//!
//! Three types in a one-way sequence, and the sequence is the enforcement:
//!
//! ```text
//! &str  --HttpsUrl::parse-->  HttpsUrl  --resolve_and_pin(&Allowlist,&DenyList)-->  PinnedDestination
//!         (pin 1)                          (pins 6 then 2)                            (consumed by deliver)
//! ```
//!
//! There is no other route into a `PinnedDestination`. It has no public
//! fields, no public constructor, no `Clone`, no `Default`, and nothing that
//! turns bytes into one. A caller who has a `PinnedDestination` therefore
//! holds *evidence* that all three pins were applied, in that order, on this
//! resolution — which is the property §7b actually wants and which a boolean
//! returned by a `validate()` function does not give you.
//!
//! # Why `HttpsUrl` is hand-parsed
//!
//! A general URL parser is a large surface with a long history of
//! disagreement about edge cases, and every disagreement between the parser
//! that *checks* a URL and the code that *uses* it is a bypass. This parser
//! accepts a deliberately small language — `https://host[:port]/path[?query]`
//! — and the fields it produces are the only fields the client reads. There
//! is no second interpretation for a `\` to differ about, no userinfo section
//! to smuggle a host through, and no fragment.
//!
//! # The canonical form is the thing the owner signs
//!
//! [`HttpsUrl::canonical`] is a **fixed rendering**: lowercase host, the
//! default port omitted, a path that is always present and always begins with
//! `/`. Layer 2's owner signature is over exactly those bytes, so a platform
//! that stores `https://Example.COM:443/hook` and later serves
//! `https://example.com/hook` has not changed anything the signature covers —
//! and a platform that changes the *host* has broken the signature, which is
//! the whole point of the layer. Parsing is idempotent on the canonical form,
//! and `canonical_is_a_fixed_point` asserts it: if it were not, a round trip
//! through storage could invalidate an honest destination.

use std::net::SocketAddr;

use crate::deny::DenyList;
use crate::refusal::Refusal;

/// A destination URL that is `https`, and cannot be anything else.
///
/// Pin 1. The only constructor is [`HttpsUrl::parse`] and it refuses every
/// other scheme. There is no `HttpUrl`, no scheme field, and no way to build
/// one of these from parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsUrl {
    host: String,
    port: u16,
    /// Always begins with `/`. Carries the query string if there was one.
    path: String,
}

/// `https` is 443 and there is no other scheme here, so there is one default.
pub const HTTPS_PORT: u16 = 443;

impl HttpsUrl {
    /// Parse, or say why not.
    ///
    /// Refused, each with a reason worth stating:
    /// - any scheme but `https` — pin 1, and it is the whole of pin 1;
    /// - a userinfo section (`https://a@b/`) — the browser-address-bar
    ///   confusion, and a way to make a checker and a client disagree about
    ///   which substring is the host;
    /// - a fragment — it never travels on the wire, so a destination that
    ///   carries one is a destination whose owner signed bytes that do not
    ///   describe the request;
    /// - an empty host, a bracketed IPv6 literal with no closing bracket, a
    ///   port that is not a number or is zero;
    /// - a control character or whitespace anywhere — CR and LF in a URL are
    ///   request smuggling, and this is where they stop.
    pub fn parse(raw: &str) -> Result<HttpsUrl, Refusal> {
        if raw.chars().any(|c| c.is_control() || c == ' ') {
            return Err(Refusal::MalformedUrl {
                detail: "contains whitespace or a control character".into(),
            });
        }
        let rest = match raw.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => rest,
            Some((scheme, _)) => {
                return Err(Refusal::NotHttps {
                    detail: format!("scheme `{scheme}`"),
                })
            }
            None => {
                return Err(Refusal::NotHttps {
                    detail: "no scheme".into(),
                })
            }
        };
        if rest.contains('#') {
            return Err(Refusal::MalformedUrl {
                detail: "a fragment never reaches the destination".into(),
            });
        }
        let (authority, path) = match rest.find(['/', '?']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if authority.contains('@') {
            return Err(Refusal::MalformedUrl {
                detail: "a userinfo section is not accepted".into(),
            });
        }
        if authority.contains('\\') {
            return Err(Refusal::MalformedUrl {
                detail: "a backslash in the authority".into(),
            });
        }
        let (host, port) = split_authority(authority)?;
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Ok(HttpsUrl { host, port, path })
    }

    /// The hostname, lowercased. What the certificate is validated against
    /// and what goes in SNI and `Host:` — never what is dialled.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Request target, always beginning with `/`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The fixed rendering the owner signature covers. See the module doc.
    pub fn canonical(&self) -> String {
        let host = if self.host.contains(':') {
            // An IPv6 literal is re-bracketed; `parse` stripped the brackets.
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == HTTPS_PORT {
            format!("https://{host}{}", self.path)
        } else {
            format!("https://{host}:{}{}", self.port, self.path)
        }
    }
}

fn split_authority(authority: &str) -> Result<(String, u16), Refusal> {
    let malformed = |d: &str| Refusal::MalformedUrl { detail: d.into() };
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| malformed("unclosed IPv6 literal"))?;
        if host.is_empty() {
            return Err(malformed("empty host"));
        }
        let port = match tail {
            "" => HTTPS_PORT,
            t => parse_port(
                t.strip_prefix(':')
                    .ok_or_else(|| malformed("trailing junk after `]`"))?,
            )?,
        };
        return Ok((host.to_ascii_lowercase(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.is_empty() {
                return Err(malformed("empty host"));
            }
            Ok((host.to_ascii_lowercase(), parse_port(port)?))
        }
        None => {
            if authority.is_empty() {
                return Err(malformed("empty host"));
            }
            Ok((authority.to_ascii_lowercase(), HTTPS_PORT))
        }
    }
}

fn parse_port(s: &str) -> Result<u16, Refusal> {
    match s.parse::<u16>() {
        Ok(0) | Err(_) => Err(Refusal::MalformedUrl {
            detail: format!("`{s}` is not a port"),
        }),
        Ok(p) => Ok(p),
    }
}

/// Pin 6: the set of hosts a space has consented to deliver to.
///
/// §7b: *"adding a host is a new consent — not a config edit."* This type
/// carries no mutation method for that reason: an [`Allowlist`] is built from
/// the space's consented grant state at the moment a delivery is prepared,
/// and there is nothing to call that would widen one in place.
///
/// For attestation the allowlist is **exactly the owner-signed destination**
/// and nothing else, which is what [`Allowlist::exactly`] means. That is a
/// real narrowing rather than a formality: it is what makes a compromise of
/// the delivery queue unable to redirect a delivery even to another host the
/// same space uses for something else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowlist {
    hosts: Vec<String>,
}

impl Allowlist {
    /// One host, the attestation case.
    pub fn exactly(host: &str) -> Allowlist {
        Allowlist {
            hosts: vec![host.to_ascii_lowercase()],
        }
    }

    /// Several — for the sink family, when it arrives.
    pub fn of(hosts: &[String]) -> Allowlist {
        Allowlist {
            hosts: hosts.iter().map(|h| h.to_ascii_lowercase()).collect(),
        }
    }

    /// Exact match on the lowercased hostname. **No suffix matching**, and
    /// that is deliberate: `*.example.com` in an allowlist is a promise that
    /// whoever controls one subdomain controls the list, and subdomain
    /// takeover is the most common way an allowlist stops meaning anything.
    pub fn permits(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.hosts.contains(&host)
    }

    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }
}

/// A destination that has passed all three destination pins, on this
/// resolution, and may be dialled once.
///
/// Not `Clone`, not `Copy`, no public constructor, no serialisation. See the
/// module doc: this type *is* the enforcement, and [`crate::deliver`] takes
/// it by value so the pin cannot be reused across deliveries.
#[derive(Debug)]
pub struct PinnedDestination {
    host: String,
    port: u16,
    path: String,
    addr: SocketAddr,
    /// Every address the name resolved to, kept for the operational log. Not
    /// used to dial; see `resolve_and_pin` on why *all* of them had to pass.
    resolved: Vec<SocketAddr>,
}

impl PinnedDestination {
    /// The name — for SNI, for certificate validation, for `Host:`.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// The address the socket will be opened to. This, not the hostname, is
    /// what `deliver` connects to — the half §7b says most implementations
    /// omit.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every address the name answered with, all of which passed the deny
    /// list. For the deployment's operational log.
    pub fn resolved(&self) -> &[SocketAddr] {
        &self.resolved
    }

    /// The canonical destination string, for the signature's signed material.
    pub fn canonical(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == HTTPS_PORT {
            format!("https://{host}{}", self.path)
        } else {
            format!("https://{host}:{}{}", self.port, self.path)
        }
    }
}

/// Pin 6 then pin 2: check the allowlist, resolve, check **every** answer,
/// and pin the first one.
///
/// # Every answer, not the one we picked
///
/// A hostile name can answer with two addresses — one public, one
/// `127.0.0.1`. An implementation that checks only the address it happens to
/// use is a coin flip the attacker can re-flip by retrying, and this platform
/// retries with backoff five times. So a single denied address in the answer
/// set refuses the whole resolution. The cost is a false refusal for a
/// legitimate dual-stack host with an unroutable member; the benefit is that
/// the check is not probabilistic.
///
/// # The allowlist is consulted *first*
///
/// A DNS lookup is itself an outbound request to a name the tenant chose, and
/// a resolver that logs queries is a channel. Checking the allowlist after
/// resolving would perform the lookup for a host the space never consented
/// to — the leak the allowlist exists to prevent, arriving one step early.
pub async fn resolve_and_pin(
    url: &HttpsUrl,
    allow: &Allowlist,
    deny: &DenyList,
) -> Result<PinnedDestination, Refusal> {
    pin(url, allow, deny).await
}

/// Pin a mail relay: a host and a port, with no URL and no path.
///
/// SMTP over implicit TLS (`smtp`) is not `https`, but every destination pin
/// holds for it as for a sink: the allowlist first, then every resolved
/// address through the deny list, then the first one pinned and consumed by
/// value. The host is checked by the same rules [`HttpsUrl::parse`] applies
/// to an authority, so a relay name cannot carry a userinfo section, a
/// control character or a second host.
pub async fn resolve_and_pin_host(
    host: &str,
    port: u16,
    allow: &Allowlist,
    deny: &DenyList,
) -> Result<PinnedDestination, Refusal> {
    if host.contains(['/', '?', '#', '@']) {
        return Err(Refusal::MalformedUrl {
            detail: "a relay is a host name, not a URL".into(),
        });
    }
    let authority = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let url = HttpsUrl::parse(&format!("https://{authority}"))?;
    let mut dest = pin(&url, allow, deny).await?;
    dest.path = String::new();
    Ok(dest)
}

async fn pin(
    url: &HttpsUrl,
    allow: &Allowlist,
    deny: &DenyList,
) -> Result<PinnedDestination, Refusal> {
    if !allow.permits(url.host()) {
        return Err(Refusal::NotAllowlisted {
            host: url.host().to_string(),
        });
    }
    let target = if url.host().contains(':') {
        format!("[{}]:{}", url.host(), url.port())
    } else {
        format!("{}:{}", url.host(), url.port())
    };
    let resolved: Vec<SocketAddr> = tokio::net::lookup_host(&target)
        .await
        .map_err(|_| Refusal::Unresolvable {
            host: url.host().to_string(),
        })?
        .collect();
    if resolved.is_empty() {
        return Err(Refusal::Unresolvable {
            host: url.host().to_string(),
        });
    }
    for addr in &resolved {
        if let Err(category) = deny.check_host(url.host(), addr.ip()) {
            return Err(Refusal::DeniedAddress {
                host: url.host().to_string(),
                address: addr.ip(),
                category,
            });
        }
    }
    Ok(PinnedDestination {
        host: url.host().to_string(),
        port: url.port(),
        path: url.path().to_string(),
        addr: resolved[0],
        resolved,
    })
}

/// The test seam, and the reason it is shaped the way it is.
///
/// Every pin except resolution can be proved against a hostile input with no
/// network at all. Resolution cannot: proving "a name that answers 127.0.0.1
/// is refused" needs a name that answers 127.0.0.1, and proving "the socket
/// goes to the pinned address" needs a socket. `localhost` supplies the
/// first. For the second, [`pin_for_test`] mints a `PinnedDestination`
/// directly — and it exists **only** under `cfg(test)` in this crate and
/// behind the `test-pin` feature for the integration suite, so no deployment
/// build contains a constructor that skips the checks.
#[cfg(any(test, feature = "test-pin"))]
pub fn pin_for_test(host: &str, addr: SocketAddr, path: &str) -> PinnedDestination {
    PinnedDestination {
        host: host.to_string(),
        port: addr.port(),
        path: path.to_string(),
        addr,
        resolved: vec![addr],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_is_not_a_destination() {
        let e = HttpsUrl::parse("http://example.com/hook").unwrap_err();
        assert_eq!(e.kind(), "not_https");
        // and the metadata endpoint's usual spelling gets no further.
        let e = HttpsUrl::parse("http://169.254.169.254/latest/meta-data/").unwrap_err();
        assert_eq!(e.kind(), "not_https");
    }

    #[test]
    fn every_other_scheme_is_refused_too() {
        for u in [
            "ftp://example.com/",
            "file:///etc/passwd",
            "gopher://example.com:70/",
            "//example.com/",
            "example.com/hook",
            "HTTP://example.com/",
        ] {
            assert!(HttpsUrl::parse(u).is_err(), "{u}");
        }
        // Case in the scheme is not a smuggle; `HTTPS` is https.
        assert!(HttpsUrl::parse("HTTPS://example.com/").is_ok());
    }

    #[test]
    fn userinfo_and_fragments_and_control_bytes() {
        assert!(HttpsUrl::parse("https://evil.com@example.com/").is_err());
        assert!(HttpsUrl::parse("https://example.com/x#y").is_err());
        assert!(HttpsUrl::parse("https://example.com/x\r\nHost: evil").is_err());
        assert!(HttpsUrl::parse("https://example.com\\@evil.com/").is_err());
        assert!(HttpsUrl::parse("https://example.com:0/").is_err());
        assert!(HttpsUrl::parse("https:///path").is_err());
    }

    #[test]
    fn canonical_is_a_fixed_point() {
        // If it were not, a round trip through the destination column would
        // invalidate an honest owner signature.
        for raw in [
            "https://Example.COM/hook",
            "https://example.com:443/hook",
            "https://example.com:8443/hook?x=1",
            "https://example.com",
            "https://[2606:4700::1111]/hook",
            "https://[2606:4700::1111]:8443/hook",
        ] {
            let once = HttpsUrl::parse(raw).unwrap().canonical();
            let twice = HttpsUrl::parse(&once).unwrap().canonical();
            assert_eq!(once, twice, "{raw}");
        }
        assert_eq!(
            HttpsUrl::parse("https://Example.COM:443/hook")
                .unwrap()
                .canonical(),
            "https://example.com/hook"
        );
        assert_eq!(
            HttpsUrl::parse("https://example.com").unwrap().canonical(),
            "https://example.com/"
        );
    }

    #[test]
    fn an_allowlist_does_not_do_suffix_matching() {
        let a = Allowlist::exactly("hooks.example.com");
        assert!(a.permits("hooks.example.com"));
        assert!(a.permits("HOOKS.EXAMPLE.COM"));
        assert!(!a.permits("example.com"));
        assert!(!a.permits("evil.hooks.example.com"));
        assert!(!a.permits("hooks.example.com.evil.net"));
    }

    #[tokio::test]
    async fn a_host_off_the_allowlist_is_never_resolved() {
        let url = HttpsUrl::parse("https://example.com/hook").unwrap();
        let e = resolve_and_pin(
            &url,
            &Allowlist::exactly("hooks.example.com"),
            &DenyList::categories_only(),
        )
        .await
        .unwrap_err();
        assert_eq!(e.kind(), "not_allowlisted");
    }

    #[tokio::test]
    async fn localhost_is_refused_by_the_deny_list() {
        let url = HttpsUrl::parse("https://localhost:8443/hook").unwrap();
        let e = resolve_and_pin(
            &url,
            &Allowlist::exactly("localhost"),
            &DenyList::categories_only(),
        )
        .await
        .unwrap_err();
        assert_eq!(e.kind(), "denied_address");
        assert!(!e.retryable(), "a name that answers loopback will again");
        // `localhost` answers both loopbacks, and the refusal names the first
        // denied address in the resolver's order, which the resolver does not
        // promise: this machine has returned `127.0.0.1` first on one day and
        // `::1` first on the next. Either is the truth; the test used to insist
        // on one.
        for loopback in ["127.0.0.1", "::1"] {
            assert!(
                !e.disclosable().contains(loopback),
                "the customer is told the category, never the address: {}",
                e.disclosable()
            );
        }
        assert!(
            ["127.0.0.1", "::1"]
                .iter()
                .any(|a| e.operational().contains(a)),
            "the operator is told which loopback was refused: {}",
            e.operational()
        );
    }
}
