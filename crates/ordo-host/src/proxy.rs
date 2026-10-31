//! Proxy mode: slow a busy visitor down, never refuse one (ruled 2026-10-06
//! and 2026-10-11, `public-data` ask 28, board `proxy-mode`).
//!
//! Behind a CDN or a load balancer, Ordo sees the proxy's address, so every
//! visitor shares one. The deployment owner may name the header that carries
//! the visitor's address and the proxies trusted to set it. Then a busy
//! address is slowed down, by a delay before its request runs. Nothing is
//! ever refused because of an address. The owner: *"IPs are just not
//! something we should rely on."*
//!
//! - **Off by default.** Until `ORDO_PROXY_HEADER` and `ORDO_PROXY_TRUSTED`
//!   are both set, no decision uses a client address, and the two address
//!   limits below are not read.
//! - **Three windows, each a slow-down:**
//!   - tokenless requests to `/op/` and `/blob/` per minute
//!     (`ORDO_PROXY_SLOW_AFTER`, 120). A caller with a token has quotas;
//!   - sign-ups per hour at `POST /auth/register`
//!     (`ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR`, off). It counts the
//!     sign-ups that went through;
//!   - wrong share-link secrets per minute at `POST /auth/link`
//!     (`ORDO_SHARE_LINK_MISS_LIMIT`, off). It counts the refused opens.
//!
//!   Until 2026-10-06 the last two answered `429`. They now delay instead.
//! - **The header is believed only from a trusted proxy.** From any other
//!   peer, the socket's address is the visitor's, so a spoofed header buys
//!   nothing. In a list such as `X-Forwarded-For`, the visitor is the
//!   right-most address that is not a trusted proxy.
//! - **The CDN comes first.** Its cache and page protection stop most floods
//!   before they reach Ordo. This is the line after them.
//! - **No address is logged,** and none is kept past its window (an hour for
//!   sign-ups, a minute otherwise). Monitoring counts how many requests were
//!   slowed, in `ordo_public_slowed_total{window}`.
//!
//! The delay is `ORDO_PROXY_SLOW_MS` for each further limit's worth of
//! requests in the window, up to `ORDO_PROXY_SLOW_MAX_MS`.
//!
//! | Variable | Default | Means |
//! |---|---|---|
//! | `ORDO_PROXY_HEADER` | unset (off) | the header naming the visitor, such as `CF-Connecting-IP` or `X-Forwarded-For` |
//! | `ORDO_PROXY_TRUSTED` | unset | the proxies' addresses, as a comma list of addresses or CIDR blocks |
//! | `ORDO_PROXY_SLOW_AFTER` | 120 | tokenless requests one visitor may make per minute before the delay starts; 0 never slows |
//! | `ORDO_PROXY_SLOW_MS` | 250 | the delay, in milliseconds, per further limit's worth |
//! | `ORDO_PROXY_SLOW_MAX_MS` | 5000 | the longest delay, in milliseconds |
//! | `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR` | off | sign-ups one address makes per hour before its next is slowed |
//! | `ORDO_SHARE_LINK_MISS_LIMIT` | off | wrong link secrets one address sends per minute before its next open is slowed |

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::Response;
use axum::Router;
use ordo_egress::Cidr;

pub const HEADER_ENV: &str = "ORDO_PROXY_HEADER";
pub const TRUSTED_ENV: &str = "ORDO_PROXY_TRUSTED";
pub const SLOW_AFTER_ENV: &str = "ORDO_PROXY_SLOW_AFTER";
pub const SLOW_MS_ENV: &str = "ORDO_PROXY_SLOW_MS";
pub const SLOW_MAX_ENV: &str = "ORDO_PROXY_SLOW_MAX_MS";
pub const SIGNUPS_ENV: &str = "ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR";
pub const LINK_MISSES_ENV: &str = "ORDO_SHARE_LINK_MISS_LIMIT";

/// How many visitors one window tracks at once. Past it, passed windows are
/// pruned, and a visitor who still finds no room is not slowed: the bound
/// fails toward serving, never toward refusing.
const TRACKED_MAX: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxySettings {
    pub header: String,
    pub trusted: Vec<String>,
    pub slow_after: u32,
    pub slow_ms: u64,
    pub slow_max_ms: u64,
    /// `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR`. `None` is off.
    pub signups_per_hour: Option<u32>,
    /// `ORDO_SHARE_LINK_MISS_LIMIT`. `None` is off.
    pub link_misses_per_minute: Option<u32>,
}

/// Proxy mode as read: the settings and the trusted proxy addresses.
pub type ProxyMode = (ProxySettings, Vec<Cidr>);

/// Read proxy mode. `Ok(None)` is off: neither variable is set. One without
/// the other refuses the boot, naming the one that is missing.
pub fn from_vars(
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<ProxyMode>, (&'static str, String)> {
    let get = |k| {
        var(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let (header, trusted) = match (get(HEADER_ENV), get(TRUSTED_ENV)) {
        (None, None) => return Ok(None),
        (Some(_), None) => {
            return Err((
                TRUSTED_ENV,
                format!(
                    "unset while {HEADER_ENV} is set. List the proxies trusted to set the \
                     header, such as {TRUSTED_ENV}=10.0.0.0/8, or unset {HEADER_ENV} to turn \
                     proxy mode off"
                ),
            ))
        }
        (None, Some(_)) => {
            return Err((
                HEADER_ENV,
                format!(
                    "unset while {TRUSTED_ENV} is set. Name the header your proxy sets, such \
                     as {HEADER_ENV}=CF-Connecting-IP, or unset {TRUSTED_ENV}"
                ),
            ))
        }
        (Some(h), Some(t)) => (h, t),
    };
    if axum::http::HeaderName::from_bytes(header.as_bytes()).is_err() {
        return Err((HEADER_ENV, format!("`{header}` is not a header name")));
    }
    let mut blocks = Vec::new();
    let mut names = Vec::new();
    for entry in trusted.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let block = if entry.contains('/') {
            entry.to_string()
        } else {
            match entry.parse::<IpAddr>() {
                Ok(IpAddr::V4(_)) => format!("{entry}/32"),
                Ok(IpAddr::V6(_)) => format!("{entry}/128"),
                Err(_) => {
                    return Err((
                        TRUSTED_ENV,
                        format!("`{entry}` is not an address or a CIDR block"),
                    ))
                }
            }
        };
        blocks.push(Cidr::parse(&block).map_err(|e| (TRUSTED_ENV, e))?);
        names.push(entry.to_string());
    }
    let number = |name: &'static str,
                  default: u64,
                  lo: u64,
                  hi: u64|
     -> Result<u64, (&'static str, String)> {
        match get(name) {
            None => Ok(default),
            Some(v) => v
                .parse::<u64>()
                .ok()
                .filter(|n| (lo..=hi).contains(n))
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
    };
    let limit = |name: &'static str| -> Result<Option<u32>, (&'static str, String)> {
        match get(name) {
            None => Ok(None),
            Some(v) if v.eq_ignore_ascii_case("off") => Ok(None),
            Some(v) => match v.parse::<u32>() {
                Ok(n) if (1..=1_000_000).contains(&n) => Ok(Some(n)),
                _ => Err((
                    name,
                    format!(
                        "`{v}` is `off` or a whole number from 1 to 1000000; unset it for \
                         the default, off. Over it an address is slowed, never refused"
                    ),
                )),
            },
        }
    };
    let settings = ProxySettings {
        header,
        trusted: names,
        slow_after: number(SLOW_AFTER_ENV, 120, 0, 1_000_000)? as u32,
        slow_ms: number(SLOW_MS_ENV, 250, 1, 60_000)?,
        slow_max_ms: number(SLOW_MAX_ENV, 5_000, 0, 60_000)?,
        signups_per_hour: limit(SIGNUPS_ENV)?,
        link_misses_per_minute: limit(LINK_MISSES_ENV)?,
    };
    Ok(Some((settings, blocks)))
}

/// The boot's lines for the two address limits when proxy mode is off: each
/// that is set has no effect yet, and says what turns it on.
pub fn inert_limits(is_set: impl Fn(&str) -> bool) -> Vec<String> {
    [SIGNUPS_ENV, LINK_MISSES_ENV]
        .into_iter()
        .filter(|v| is_set(v))
        .map(|v| {
            format!(
                "{v} has no effect until proxy mode is on: set {HEADER_ENV} and \
                 {TRUSTED_ENV}. Over it an address is slowed, never refused"
            )
        })
        .collect()
}

/// The visitor's address: the header's, read only from a trusted peer.
pub fn visitor(peer: IpAddr, header: Option<&str>, trusted: &[Cidr]) -> Option<IpAddr> {
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

/// Counts per address in a fixed window.
struct Window {
    span: Duration,
    counts: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl Window {
    fn new(span: Duration) -> Window {
        Window {
            span,
            counts: Mutex::new(HashMap::new()),
        }
    }

    /// The count for `ip` in its current window, after adding `add`.
    fn add(&self, ip: IpAddr, add: u32, now: Instant) -> u32 {
        let mut w = self.counts.lock().unwrap_or_else(|p| p.into_inner());
        if w.len() >= TRACKED_MAX && !w.contains_key(&ip) {
            let span = self.span;
            w.retain(|_, (start, _)| now.saturating_duration_since(*start) < span);
            if w.len() >= TRACKED_MAX {
                return 0;
            }
        }
        let slot = w.entry(ip).or_insert((now, 0));
        if now.saturating_duration_since(slot.0) >= self.span {
            *slot = (now, 0);
        }
        slot.1 = slot.1.saturating_add(add);
        slot.1
    }
}

/// Which window a request belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A tokenless call to `/op/` or `/blob/`: counted as it arrives.
    Visit,
    /// `POST /auth/register`: a sign-up that went through counts.
    Signup,
    /// `POST /auth/link`: a refused open counts.
    LinkMiss,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Visit => "visit",
            Kind::Signup => "signup",
            Kind::LinkMiss => "link_miss",
        }
    }
}

/// The three windows and the delay they earn.
pub struct Pacer {
    settings: ProxySettings,
    trusted: Vec<Cidr>,
    visits: Window,
    signups: Window,
    link_misses: Window,
    metrics: Option<Arc<crate::metrics::Metrics>>,
}

impl Pacer {
    pub fn new(
        settings: ProxySettings,
        trusted: Vec<Cidr>,
        metrics: Option<Arc<crate::metrics::Metrics>>,
    ) -> Pacer {
        Pacer {
            settings,
            trusted,
            visits: Window::new(Duration::from_secs(60)),
            signups: Window::new(Duration::from_secs(3600)),
            link_misses: Window::new(Duration::from_secs(60)),
            metrics,
        }
    }

    pub fn settings(&self) -> &ProxySettings {
        &self.settings
    }

    /// `slow_ms` for each further `limit`'s worth past `limit`, capped.
    fn delay_for(&self, count: u32, limit: u32) -> Duration {
        if limit == 0 || count <= limit {
            return Duration::ZERO;
        }
        let steps = 1 + u64::from((count - limit - 1) / limit);
        Duration::from_millis(
            self.settings
                .slow_ms
                .saturating_mul(steps)
                .min(self.settings.slow_max_ms),
        )
    }

    /// Count one tokenless request from `ip` and say how long it waits.
    pub fn delay_at(&self, ip: IpAddr, now: Instant) -> Duration {
        if self.settings.slow_after == 0 {
            return Duration::ZERO;
        }
        let n = self.visits.add(ip, 1, now);
        self.delay_for(n, self.settings.slow_after)
    }

    /// How long a sign-up from `ip` waits, by the sign-ups it already made
    /// this hour. Counting is [`Pacer::signed_up`], after one goes through.
    pub fn signup_delay_at(&self, ip: IpAddr, now: Instant) -> Duration {
        match self.settings.signups_per_hour {
            None => Duration::ZERO,
            Some(limit) => self.delay_for(self.signups.add(ip, 0, now) + 1, limit),
        }
    }

    pub fn signed_up(&self, ip: IpAddr, now: Instant) {
        if self.settings.signups_per_hour.is_some() {
            self.signups.add(ip, 1, now);
        }
    }

    /// How long a link open from `ip` waits, by its misses this minute.
    pub fn link_delay_at(&self, ip: IpAddr, now: Instant) -> Duration {
        match self.settings.link_misses_per_minute {
            None => Duration::ZERO,
            Some(limit) => self.delay_for(self.link_misses.add(ip, 0, now) + 1, limit),
        }
    }

    pub fn link_missed(&self, ip: IpAddr, now: Instant) {
        if self.settings.link_misses_per_minute.is_some() {
            self.link_misses.add(ip, 1, now);
        }
    }

    fn kind(&self, request: &Request) -> Option<Kind> {
        let path = request.uri().path();
        let post = request.method() == Method::POST;
        if post && path == "/auth/register" && self.settings.signups_per_hour.is_some() {
            return Some(Kind::Signup);
        }
        if post && path == "/auth/link" && self.settings.link_misses_per_minute.is_some() {
            return Some(Kind::LinkMiss);
        }
        let tokenless = !request.headers().contains_key("authorization");
        (tokenless
            && request.method() != Method::OPTIONS
            && (path.starts_with("/op/") || path.starts_with("/blob/")))
        .then_some(Kind::Visit)
    }
}

/// Wrap the door in the pacer. `None` returns it unchanged: proxy mode off.
pub fn apply(router: Router, pacer: Option<Arc<Pacer>>) -> Router {
    match pacer {
        None => router,
        Some(p) => router.layer(from_fn_with_state(p, pace)),
    }
}

async fn pace(State(pacer): State<Arc<Pacer>>, request: Request, next: Next) -> Response {
    let Some(kind) = pacer.kind(&request) else {
        return next.run(request).await;
    };
    let Some(peer) = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
    else {
        return next.run(request).await;
    };
    let header = request
        .headers()
        .get(pacer.settings.header.as_str())
        .and_then(|v| v.to_str().ok());
    let Some(ip) = visitor(peer, header, &pacer.trusted) else {
        return next.run(request).await;
    };
    let now = Instant::now();
    let wait = match kind {
        Kind::Visit => pacer.delay_at(ip, now),
        Kind::Signup => pacer.signup_delay_at(ip, now),
        Kind::LinkMiss => pacer.link_delay_at(ip, now),
    };
    if !wait.is_zero() {
        if let Some(m) = &pacer.metrics {
            m.incr("ordo_public_slowed_total", &[("window", kind.label())]);
        }
        tokio::time::sleep(wait).await;
    }
    let response = next.run(request).await;
    match (kind, response.status()) {
        (Kind::Signup, s) if s.is_success() => pacer.signed_up(ip, Instant::now()),
        (Kind::LinkMiss, StatusCode::FORBIDDEN) => pacer.link_missed(ip, Instant::now()),
        _ => {}
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let m: Vec<_> = pairs.to_vec();
        move |k: &str| m.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn settings(slow_after: u32) -> ProxySettings {
        ProxySettings {
            header: "X-Forwarded-For".into(),
            trusted: vec![],
            slow_after,
            slow_ms: 100,
            slow_max_ms: 250,
            signups_per_hour: None,
            link_misses_per_minute: None,
        }
    }

    #[test]
    fn off_unless_both_are_set_and_each_names_the_other() {
        assert!(from_vars(&vars(&[])).unwrap().is_none());
        let (var, detail) = from_vars(&vars(&[(HEADER_ENV, "CF-Connecting-IP")])).unwrap_err();
        assert_eq!(var, TRUSTED_ENV);
        assert!(detail.contains("ORDO_PROXY_TRUSTED=10.0.0.0/8"), "{detail}");
        let (var, detail) = from_vars(&vars(&[(TRUSTED_ENV, "10.0.0.0/8")])).unwrap_err();
        assert_eq!(var, HEADER_ENV);
        assert!(
            detail.contains("ORDO_PROXY_HEADER=CF-Connecting-IP"),
            "{detail}"
        );
    }

    #[test]
    fn the_settings_are_read_with_their_defaults_and_bounds() {
        let (s, blocks) = from_vars(&vars(&[
            (HEADER_ENV, "X-Forwarded-For"),
            (TRUSTED_ENV, "10.0.0.0/8, 192.0.2.7"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!((s.slow_after, s.slow_ms, s.slow_max_ms), (120, 250, 5000));
        assert_eq!((s.signups_per_hour, s.link_misses_per_minute), (None, None));
        assert_eq!(blocks.len(), 2);
        let (var, _) = from_vars(&vars(&[
            (HEADER_ENV, "X-Forwarded-For"),
            (TRUSTED_ENV, "not-an-address"),
        ]))
        .unwrap_err();
        assert_eq!(var, TRUSTED_ENV);
        let (var, detail) = from_vars(&vars(&[
            (HEADER_ENV, "X-Forwarded-For"),
            (TRUSTED_ENV, "10.0.0.0/8"),
            (SLOW_MS_ENV, "0"),
        ]))
        .unwrap_err();
        assert_eq!(var, SLOW_MS_ENV);
        assert!(detail.contains("default, 250"), "{detail}");
    }

    #[test]
    fn the_address_limits_are_off_by_default_and_read_only_in_proxy_mode() {
        let on = [(HEADER_ENV, "X-Forwarded-For"), (TRUSTED_ENV, "10.0.0.0/8")];
        let (s, _) = from_vars(&vars(&[
            on[0],
            on[1],
            (SIGNUPS_ENV, "5"),
            (LINK_MISSES_ENV, "off"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(
            (s.signups_per_hour, s.link_misses_per_minute),
            (Some(5), None)
        );
        let (var, detail) =
            from_vars(&vars(&[on[0], on[1], (LINK_MISSES_ENV, "lots")])).unwrap_err();
        assert_eq!(var, LINK_MISSES_ENV);
        assert!(detail.contains("never refused"), "{detail}");
        // Proxy mode off: not read, so nonsense does not refuse the boot,
        // and the boot says what would turn them on.
        assert!(from_vars(&vars(&[(SIGNUPS_ENV, "nonsense")]))
            .unwrap()
            .is_none());
        let lines = inert_limits(|v| v == SIGNUPS_ENV || v == LINK_MISSES_ENV);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.contains(HEADER_ENV)), "{lines:?}");
        assert!(inert_limits(|_| false).is_empty());
    }

    #[test]
    fn the_header_is_believed_only_from_a_trusted_proxy() {
        let trusted = vec![Cidr::parse("10.0.0.0/8").unwrap()];
        assert_eq!(
            visitor(ip("10.0.0.1"), Some("203.0.113.9, 10.0.0.2"), &trusted),
            Some(ip("203.0.113.9"))
        );
        // A visitor who prepends a fake address does not move it.
        assert_eq!(
            visitor(ip("10.0.0.1"), Some("198.51.100.1, 203.0.113.9"), &trusted),
            Some(ip("203.0.113.9"))
        );
        // From anyone else, the header is ignored.
        assert_eq!(
            visitor(ip("203.0.113.50"), Some("198.51.100.1"), &trusted),
            Some(ip("203.0.113.50"))
        );
        assert_eq!(visitor(ip("10.0.0.1"), None, &trusted), None);
    }

    #[test]
    fn a_busy_visitor_is_slowed_more_as_it_goes_and_never_past_the_cap() {
        let p = Pacer::new(settings(2), vec![], None);
        let t = Instant::now();
        let a = ip("203.0.113.9");
        let waits: Vec<u64> = (0..7)
            .map(|_| p.delay_at(a, t).as_millis() as u64)
            .collect();
        assert_eq!(waits, [0, 0, 100, 100, 200, 200, 250]);
        assert_eq!(p.delay_at(ip("203.0.113.10"), t), Duration::ZERO);
        assert_eq!(p.delay_at(a, t + Duration::from_secs(61)), Duration::ZERO);
    }

    #[test]
    fn zero_never_slows() {
        let p = Pacer::new(settings(0), vec![], None);
        for _ in 0..1000 {
            assert_eq!(
                p.delay_at(ip("203.0.113.9"), Instant::now()),
                Duration::ZERO
            );
        }
    }

    #[test]
    fn sign_ups_past_the_hourly_limit_are_slowed_and_the_hour_forgives() {
        let p = Pacer::new(
            ProxySettings {
                signups_per_hour: Some(2),
                ..settings(120)
            },
            vec![],
            None,
        );
        let t = Instant::now();
        let a = ip("203.0.113.9");
        for _ in 0..2 {
            assert_eq!(p.signup_delay_at(a, t), Duration::ZERO);
            p.signed_up(a, t);
        }
        assert_eq!(p.signup_delay_at(a, t), Duration::from_millis(100));
        assert_eq!(p.signup_delay_at(ip("203.0.113.10"), t), Duration::ZERO);
        assert_eq!(
            p.signup_delay_at(a, t + Duration::from_secs(3601)),
            Duration::ZERO
        );
    }

    #[test]
    fn link_misses_past_the_limit_slow_the_next_open() {
        let p = Pacer::new(
            ProxySettings {
                link_misses_per_minute: Some(3),
                ..settings(120)
            },
            vec![],
            None,
        );
        let t = Instant::now();
        let a = ip("203.0.113.9");
        for _ in 0..3 {
            assert_eq!(p.link_delay_at(a, t), Duration::ZERO);
            p.link_missed(a, t);
        }
        assert_eq!(p.link_delay_at(a, t), Duration::from_millis(100));
        assert_eq!(
            p.link_delay_at(a, t + Duration::from_secs(61)),
            Duration::ZERO
        );
        // Off counts nothing and slows nothing.
        let off = Pacer::new(settings(120), vec![], None);
        for _ in 0..50 {
            off.link_missed(a, t);
        }
        assert_eq!(off.link_delay_at(a, t), Duration::ZERO);
    }
}
