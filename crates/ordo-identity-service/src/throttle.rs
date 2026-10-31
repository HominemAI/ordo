//! Guessing limits, shared by every instance (`plan-identity-service.md`
//! 2.1).
//!
//! A fixed window, so many tries in it, then a refusal that says when to
//! try again. One window per kind of secret a stranger could guess: invite
//! links, recovery codes, CLI codes and machine sign-ins. Counted per kind
//! and per source address rather than per value presented, because a
//! guesser presents a new value every try. A second, much higher count per
//! kind for everyone together only stops a flood (owner, 2026-10-27): the
//! secrets are long random codes, so no address can guess one, and one
//! stranger must not be able to shut a kind of sign-in for everyone.
//!
//! The count lives in the service's database (`guess_counts`), as the
//! door's lives in `$guesscount`, so two instances behind one address share
//! one count. Each try is counted in one guarded statement, before the
//! secret is checked, so a burst sent at once cannot slip under the limit.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use ordo_exec::{sql_lit, Pool};

use crate::config::Limits;
use crate::error::IdentityError;
use crate::server::Service;
use crate::store::num;

/// The window.
pub const ATTEMPT_WINDOW_SECS: u64 = 60;

/// The `source` of the shared row: no address, the ceiling for everyone.
const SHARED: &str = "";
/// The `source` when no address is known (a call with no socket, as a
/// test makes). Every such call shares one count.
const UNKNOWN: &str = "unknown";

/// Who is trying: the visitor's address, as text.
///
/// Taken the way the door takes it (`ordo-host` `proxy.rs`): the socket's
/// peer, unless proxy mode is on (`ORDO_PROXY_HEADER` and
/// `ORDO_PROXY_TRUSTED`) and the peer is a trusted proxy. Then the visitor
/// is the right-most address in the header that is not itself a trusted
/// proxy. From any other peer the header is ignored, so a spoofed one buys
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source(pub String);

impl Source {
    /// No address known.
    pub fn unknown() -> Source {
        Source(UNKNOWN.into())
    }

    /// The visitor behind `peer`, given the header's text and the proxy
    /// settings.
    pub fn of(peer: IpAddr, header: Option<&str>, proxy: Option<&Proxy>) -> Source {
        let Some(p) = proxy else {
            return Source(peer.to_string());
        };
        match visitor(peer, header, &p.trusted) {
            Some(ip) => Source(ip.to_string()),
            // A trusted proxy sent no usable header: its own address.
            None => Source(peer.to_string()),
        }
    }
}

impl FromRequestParts<Service> for Source {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Service,
    ) -> Result<Source, Self::Rejection> {
        let Some(peer) = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
        else {
            return Ok(Source::unknown());
        };
        let proxy = state.config().proxy.as_ref();
        let header = proxy.and_then(|p| {
            parts
                .headers
                .get(p.header.as_str())
                .and_then(|v| v.to_str().ok())
        });
        Ok(Source::of(peer, header, proxy))
    }
}

/// Proxy mode, as the door reads it: the header that names the visitor and
/// the proxies trusted to set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    pub header: String,
    pub trusted: Vec<Cidr>,
}

/// An address block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: IpAddr,
    bits: u8,
}

impl Cidr {
    /// `10.0.0.0/8`, or a bare address (a block of 1).
    pub fn parse(text: &str) -> Result<Cidr, String> {
        let bad = || format!("`{text}` is not an address or a CIDR block");
        let (addr, bits) = match text.split_once('/') {
            Some((a, b)) => (a, Some(b.parse::<u8>().map_err(|_| bad())?)),
            None => (text, None),
        };
        let net: IpAddr = addr.parse().map_err(|_| bad())?;
        let max = if net.is_ipv4() { 32 } else { 128 };
        let bits = bits.unwrap_or(max);
        if bits > max {
            return Err(bad());
        }
        Ok(Cidr { net, bits })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        fn prefix(a: &[u8], b: &[u8], bits: u8) -> bool {
            let whole = usize::from(bits / 8);
            if a[..whole] != b[..whole] {
                return false;
            }
            let rest = bits % 8;
            rest == 0 || (a[whole] ^ b[whole]) >> (8 - rest) == 0
        }
        match (self.net, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) => prefix(&n.octets(), &i.octets(), self.bits),
            (IpAddr::V6(n), IpAddr::V6(i)) => prefix(&n.octets(), &i.octets(), self.bits),
            _ => false,
        }
    }
}

/// The door's rule: a peer that is not a trusted proxy is the visitor; from
/// a trusted proxy, the right-most listed address that is not a trusted
/// proxy.
fn visitor(peer: IpAddr, header: Option<&str>, trusted: &[Cidr]) -> Option<IpAddr> {
    let is_trusted = |ip: IpAddr| trusted.iter().any(|c| c.contains(ip));
    if !is_trusted(peer) {
        return Some(peer);
    }
    let list: Vec<IpAddr> = header?
        .split(',')
        .filter_map(|a| a.trim().parse::<IpAddr>().ok())
        .collect();
    list.iter()
        .rev()
        .find(|ip| !is_trusted(**ip))
        .or_else(|| list.first())
        .copied()
}

/// What is being tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Try {
    Invite,
    RecoveryCode,
    CliCode,
    MachineSignIn,
}

impl Try {
    fn word(self) -> &'static str {
        match self {
            Try::Invite => "invite",
            Try::RecoveryCode => "recovery-code",
            Try::CliCode => "cli-code",
            Try::MachineSignIn => "machine-sign-in",
        }
    }
}

/// Count one try at `now` against the visitor's own limit and then the
/// shared ceiling, or refuse with when to try again.
///
/// The address is counted first. A try refused for its address is not
/// counted at the ceiling, so one flooding address cannot use up the
/// ceiling that everyone else shares.
pub async fn attempt(
    pool: &Pool,
    handle: &str,
    kind: Try,
    source: &Source,
    limits: &Limits,
    now: u64,
) -> Result<(), IdentityError> {
    count(pool, handle, kind, &source.0, limits.per_address, now).await?;
    count(pool, handle, kind, SHARED, limits.shared, now).await
}

async fn count(
    pool: &Pool,
    handle: &str,
    kind: Try,
    source: &str,
    max: u64,
    now: u64,
) -> Result<(), IdentityError> {
    let w = ATTEMPT_WINDOW_SECS;
    let rows = pool
        .rows(&format!(
            "INSERT INTO guess_counts (deployment, kind, source, window_start, tries, created_at) \
             VALUES ({d}, {k}, {s}, {now}, 1, {now}) \
             ON CONFLICT (deployment, kind, source) DO UPDATE SET \
               window_start = CASE WHEN guess_counts.window_start + {w} <= {now} \
                 THEN {now} ELSE guess_counts.window_start END, \
               tries = CASE WHEN guess_counts.window_start + {w} <= {now} \
                 THEN 1 ELSE guess_counts.tries + 1 END \
             RETURNING window_start, tries",
            d = sql_lit(handle),
            k = sql_lit(kind.word()),
            s = sql_lit(source),
        ))
        .await?;
    let (start, tries) = rows
        .first()
        .map(|r| (num(r, 0).unwrap_or(now), num(r, 1).unwrap_or(0)))
        .unwrap_or((now, 0));
    if tries > max {
        return Err(IdentityError::Throttled {
            retry_after_secs: start.saturating_add(w).saturating_sub(now).max(1),
        });
    }
    Ok(())
}
