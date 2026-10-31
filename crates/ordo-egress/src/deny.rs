//! Pin 2's second half: which resolved addresses are inside the perimeter.
//!
//! §7b names the categories — loopback, link-local (169.254.0.0/16, fe80::/10,
//! fd00::/8), RFC1918, CGNAT, and the deployment's own pod/service CIDRs — and
//! `implementation.md` adds the split that matters: **"the deny-list CIDRs are
//! deployment config (a self-hosted cluster's pod range differs), the
//! *categories* are not."** So [`DenyList`] has one configurable field and no
//! way to switch a category off. There is no `allow_loopback`, and a test that
//! wants one uses a listener on a public-shaped address in a fake resolver.
//!
//! # The three bypasses this file exists to close
//!
//! A deny list over IPv4 ranges is easy. A deny list that survives contact
//! with a hostile hostname is a different exercise, and the interesting part
//! is that all three of these evade a *correct* IPv4 range check by never
//! being an IPv4 address at the moment it runs.
//!
//! **IPv4-mapped IPv6.** `::ffff:127.0.0.1` is an `IpAddr::V6`. Rust's
//! [`std::net::Ipv6Addr::is_loopback`] answers `false` for it — correctly,
//! since `::1` is the IPv6 loopback and this is not it — and the socket layer
//! then connects to 127.0.0.1 anyway. Every address is [canonicalised](canonical)
//! before any predicate sees it, so the mapped form is checked as the IPv4
//! address it will actually reach.
//!
//! **NAT64.** `64:ff9b::/96` is the well-known prefix a NAT64 gateway
//! translates back to IPv4 — so `64:ff9b::7f00:1` is 127.0.0.1 with two extra
//! steps, on any network that has such a gateway, which is most mobile
//! networks and a growing number of clusters. It is unmapped for the same
//! reason and by the same function.
//!
//! **IPv6 unique-local under an IPv4 mental model.** §7b lists `fd00::/8`,
//! which is *nearly* right — the standard is `fc00::/7`, of which `fd00::/8`
//! is the half in actual use. The wider prefix is what this file checks;
//! narrowing it to the document's letter would leave `fc00::/8` reachable,
//! and "the spec said fd00" is not a thing to tell an incident review. This
//! is a **widening of a `[pinned]` list, in the direction the pin argues
//! for**, and it is recorded in `corpus-rulings.md` rather than done quietly.
//!
//! # What is denied beyond §7b's list, and why
//!
//! `is_global` is not stable on `IpAddr`, so this is an explicit enumeration
//! rather than a negation, and an explicit enumeration has to be complete or
//! it is worse than useless. Beyond the five categories §7b names:
//! `0.0.0.0/8` (which on Linux reaches localhost), `240.0.0.0/4` reserved,
//! `255.255.255.255`, multicast in both families, IPv4 benchmarking and
//! documentation ranges, `2002::/16` (6to4, which embeds an IPv4 address the
//! same way NAT64 does — and is *rejected* rather than unmapped, because a
//! 6to4 relay is not a thing this platform should be posting through at all),
//! and IPv6 documentation space.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Which promise a refused address broke. Disclosable to the customer; the
/// address itself is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeniedCategory {
    /// 127.0.0.0/8, ::1, and 0.0.0.0/8 — which reaches localhost on Linux.
    Loopback,
    /// 169.254.0.0/16 and fe80::/10. **This is the cloud metadata endpoint**,
    /// and it is the single address §7b was written about.
    LinkLocal,
    /// RFC1918: 10/8, 172.16/12, 192.168/16.
    Private,
    /// RFC6598 carrier-grade NAT, 100.64.0.0/10.
    Cgnat,
    /// fc00::/7 unique-local.
    UniqueLocal,
    /// Multicast and broadcast, both families.
    Multicast,
    /// Reserved, benchmarking, documentation, 6to4 — address space a
    /// deployment has no business posting to.
    Reserved,
    /// This deployment's own pod/service CIDRs — the one configurable half.
    DeploymentRange,
}

impl DeniedCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            DeniedCategory::Loopback => "loopback",
            DeniedCategory::LinkLocal => "link-local",
            DeniedCategory::Private => "private",
            DeniedCategory::Cgnat => "carrier-grade NAT",
            DeniedCategory::UniqueLocal => "unique-local",
            DeniedCategory::Multicast => "multicast",
            DeniedCategory::Reserved => "reserved",
            DeniedCategory::DeploymentRange => "deployment-internal",
        }
    }
}

/// One CIDR block, either family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    base: IpAddr,
    bits: u8,
}

impl Cidr {
    /// `10.0.0.0/8`, `fd00::/8`. Refuses a prefix length wider than the
    /// family allows, and refuses to normalise silently: `10.1.0.0/8` is a
    /// typo, and a deny list that quietly widens a typo into `10.0.0.0/8` is
    /// a deny list nobody can review.
    pub fn parse(s: &str) -> Result<Cidr, String> {
        let (addr, len) = s
            .split_once('/')
            .ok_or_else(|| format!("`{s}` is not a CIDR block (no `/`)"))?;
        let base: IpAddr = addr
            .parse()
            .map_err(|_| format!("`{addr}` is not an IP address"))?;
        let bits: u8 = len
            .parse()
            .map_err(|_| format!("`{len}` is not a prefix length"))?;
        let max = if base.is_ipv4() { 32 } else { 128 };
        if bits > max {
            return Err(format!("/{bits} is wider than the address family allows"));
        }
        let c = Cidr { base, bits };
        if !c.contains_exact_base() {
            return Err(format!(
                "`{s}` has host bits set; write the network address, not an address inside it"
            ));
        }
        Ok(c)
    }

    fn contains_exact_base(&self) -> bool {
        masked(self.base, self.bits) == self.base
    }

    /// Membership, after canonicalisation of the *candidate* — so a pod CIDR
    /// written in IPv4 still catches an `::ffff:` form of an address in it.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = canonical(ip);
        match (self.base, ip) {
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => {
                masked(ip, self.bits) == self.base
            }
            _ => false,
        }
    }
}

fn masked(ip: IpAddr, bits: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let n = u32::from(v4);
            let m = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            IpAddr::V4(Ipv4Addr::from(n & m))
        }
        IpAddr::V6(v6) => {
            let n = u128::from(v6);
            let m = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            IpAddr::V6(Ipv6Addr::from(n & m))
        }
    }
}

/// Reduce an address to the thing a socket will actually reach.
///
/// IPv4-mapped (`::ffff:a.b.c.d`) and NAT64 (`64:ff9b::a.b.c.d`) forms both
/// become their IPv4 address. Everything else is returned unchanged. This is
/// the function that stands between a correct range check and a bypass; see
/// the module doc.
pub fn canonical(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else { return ip };
    let n = u128::from(v6);
    // ::ffff:0:0/96 — IPv4-mapped.
    if n >> 32 == 0x0000_0000_0000_0000_0000_ffff {
        return IpAddr::V4(Ipv4Addr::from(n as u32));
    }
    // ::/96 with a non-zero low word — the deprecated IPv4-compatible form.
    // `::` and `::1` are left alone; they are handled as themselves.
    if n >> 32 == 0 && (n as u32) > 1 {
        return IpAddr::V4(Ipv4Addr::from(n as u32));
    }
    // 64:ff9b::/96 — the well-known NAT64 prefix.
    if n >> 32 == 0x0064_ff9b_0000_0000_0000_0000 {
        return IpAddr::V4(Ipv4Addr::from(n as u32));
    }
    ip
}

/// The perimeter.
///
/// Categories are compiled in and cannot be disabled; `deployment` is the
/// configurable half `implementation.md` requires.
#[derive(Debug, Clone, Default)]
pub struct DenyList {
    deployment: Vec<Cidr>,
    /// `ORDO_EGRESS_PRIVATE_HOSTS`: exact host names, IP addresses and CIDR
    /// ranges, let through the private categories. See [`DenyList::with_private_hosts`].
    private_hosts: Vec<PrivateHost>,
    /// See [`DenyList::for_tenant_providers`].
    tenant_reach: bool,
    /// See [`DenyList::permitting_loopback_for_tests`]. Behind `test-pin`, so
    /// the field does not exist in a deployment build.
    #[cfg(feature = "test-pin")]
    permit_loopback: bool,
}

impl DenyList {
    /// The categories alone. Correct for a single-node deployment with no
    /// private pod network; a cluster adds its ranges.
    pub fn categories_only() -> DenyList {
        DenyList::default()
    }

    /// Add this deployment's pod/service CIDRs. `ORDO_EGRESS_DENY_CIDRS`,
    /// comma-separated, is the environment spelling; the parse is strict so a
    /// typo fails boot rather than silently leaving a range reachable.
    pub fn with_deployment_cidrs(mut self, cidrs: &[Cidr]) -> DenyList {
        self.deployment.extend_from_slice(cidrs);
        self
    }

    /// The list a tenant's own sign-in provider is reached through
    /// (`plan-tenant-providers.md` §9; owner, 2026-10-09: the tenant decides
    /// where its provider is).
    ///
    /// The private ranges are let through: RFC 1918, unique-local and
    /// carrier-grade NAT. Still refused: loopback, link-local, multicast,
    /// reserved space, this deployment's own ranges, and every cloud
    /// metadata address, even inside a range that is let through. The
    /// deployment's `ORDO_EGRESS_PRIVATE_HOSTS` is dropped: it is the app
    /// developer's list for the deployment's own egress, never a tenant's.
    pub fn for_tenant_providers(&self) -> DenyList {
        DenyList {
            deployment: self.deployment.clone(),
            private_hosts: Vec::new(),
            tenant_reach: true,
            #[cfg(feature = "test-pin")]
            permit_loopback: self.permit_loopback,
        }
    }

    /// Parse a comma-separated list. Empty string is an empty list, not an
    /// error — a single-node deployment legitimately has none.
    pub fn parse_cidrs(s: &str) -> Result<Vec<Cidr>, String> {
        s.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(Cidr::parse)
            .collect()
    }

    /// The deployment ranges, for the consent page's disclosure.
    pub fn deployment_cidrs(&self) -> &[Cidr] {
        &self.deployment
    }

    /// Let these entries reach a private address
    /// (`plan-backend-functions.md` §6 and Q6, `plan-auth-federated.md`
    /// §3.4, §7.6): a mail relay inside the cluster, or Mailpit on the
    /// developer's machine.
    ///
    /// An entry is an exact host name, an exact IP address or a CIDR range.
    /// A listed host (or an address in a listed range) may reach loopback,
    /// RFC1918, unique-local, carrier-grade NAT or this deployment's own
    /// ranges. It may **never** reach link-local (the metadata endpoint),
    /// the IPv6 and Alibaba metadata addresses, multicast or reserved space,
    /// even when a listed range contains them. A host not on the list is
    /// checked as before. A name matches exactly, ignoring case: no
    /// wildcard, no suffix. Resolution and pinning are unchanged.
    ///
    /// Only deployment-scoped egress the app developer configured reads
    /// this list. A space's sinks do not: a tenant must never reach a host
    /// the deployment owner let through for itself.
    pub fn with_private_hosts(mut self, hosts: &[PrivateHost]) -> DenyList {
        self.private_hosts.extend(hosts.iter().cloned());
        self
    }

    /// The deployment's deny list from its two settings,
    /// `ORDO_EGRESS_DENY_CIDRS` and `ORDO_EGRESS_PRIVATE_HOSTS`, read through
    /// `var`. The one place either is parsed: the boot reads it once, whether
    /// or not mail is on, and every outbound call (mail, OIDC, OAuth2,
    /// Turnstile) gets a clone. A bad entry names its setting.
    pub fn from_vars(
        var: &dyn Fn(&str) -> Option<String>,
    ) -> Result<DenyList, (&'static str, String)> {
        let cidrs = match var(DENY_CIDRS_ENV) {
            Some(s) => DenyList::parse_cidrs(&s).map_err(|d| (DENY_CIDRS_ENV, d))?,
            None => Vec::new(),
        };
        let hosts = match var(PRIVATE_HOSTS_ENV) {
            Some(s) => DenyList::parse_private_hosts(&s).map_err(|d| (PRIVATE_HOSTS_ENV, d))?,
            None => Vec::new(),
        };
        Ok(DenyList::categories_only()
            .with_deployment_cidrs(&cidrs)
            .with_private_hosts(&hosts))
    }

    /// Parse `ORDO_EGRESS_PRIVATE_HOSTS`: comma-separated exact host names,
    /// IP addresses or CIDR ranges. Empty is an empty list. A wildcard, a
    /// port, a scheme or a path is refused, so a typo fails the boot instead
    /// of widening the list.
    pub fn parse_private_hosts(s: &str) -> Result<Vec<PrivateHost>, String> {
        s.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|h| {
                if let Ok(ip) = h.parse::<IpAddr>() {
                    let bits = if ip.is_ipv4() { 32 } else { 128 };
                    return Ok(PrivateHost::Net(Cidr { base: ip, bits }));
                }
                if h.contains('/') {
                    return Cidr::parse(h)
                        .map(PrivateHost::Net)
                        .map_err(|e| format!("`{h}` is not a CIDR range ({e})"));
                }
                let label_ok = |l: &str| {
                    !l.is_empty()
                        && l.len() <= 63
                        && !l.starts_with('-')
                        && !l.ends_with('-')
                        && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                };
                if h.len() <= 253 && h.split('.').all(label_ok) {
                    Ok(PrivateHost::Name(h.to_ascii_lowercase()))
                } else {
                    Err(format!(
                        "`{h}` is not a host name, IP address or CIDR range (letters, digits, \
                         `-` and `.` only; no wildcard, port or scheme)"
                    ))
                }
            })
            .collect()
    }

    /// The private hosts, for the startup log.
    pub fn private_hosts(&self) -> &[PrivateHost] {
        &self.private_hosts
    }

    /// `Ok(())` if `ip`, resolved from `host`, may be dialled: [`check`],
    /// with the private categories let through for a named private host.
    ///
    /// [`check`]: DenyList::check
    pub fn check_host(&self, host: &str, ip: IpAddr) -> Result<(), DeniedCategory> {
        match self.check(ip) {
            Ok(()) => Ok(()),
            Err(
                c @ (DeniedCategory::Loopback
                | DeniedCategory::Private
                | DeniedCategory::UniqueLocal
                | DeniedCategory::Cgnat
                | DeniedCategory::DeploymentRange),
            ) => {
                if is_metadata(ip) {
                    return Err(DeniedCategory::LinkLocal);
                }
                if self.tenant_reach
                    && matches!(
                        c,
                        DeniedCategory::Private
                            | DeniedCategory::UniqueLocal
                            | DeniedCategory::Cgnat
                    )
                    && !self.deployment.iter().any(|r| r.contains(ip))
                {
                    return Ok(());
                }
                if self.private_hosts.iter().any(|h| h.matches(host, ip)) {
                    Ok(())
                } else {
                    Err(c)
                }
            }
            Err(c) => Err(c),
        }
    }

    /// **Tests only.** Permit loopback, so an integration suite can point a
    /// delivery at a listener on 127.0.0.1.
    ///
    /// Behind the `test-pin` feature, which is off in every deployment build
    /// — `holds_nothing.rs` asserts that no crate in this workspace enables
    /// it as a *non-dev* dependency, so a build that could reach this is a
    /// diff a reviewer sees.
    ///
    /// It permits **loopback only**, never link-local, and that line is the
    /// point: 169.254.169.254 is the address §7b was written about, and a
    /// test escape that opened it would be a test escape that could hide the
    /// bug the whole file exists to prevent. A test that needs the metadata
    /// endpoint reachable is a test that is proving the wrong thing.
    #[cfg(feature = "test-pin")]
    pub fn permitting_loopback_for_tests(mut self) -> DenyList {
        self.permit_loopback = true;
        self
    }

    /// `Ok(())` if this address may be dialled.
    pub fn check(&self, ip: IpAddr) -> Result<(), DeniedCategory> {
        if let Some(c) = category_of(ip) {
            #[cfg(feature = "test-pin")]
            if self.permit_loopback && c == DeniedCategory::Loopback {
                return Ok(());
            }
            return Err(c);
        }
        if self.deployment.iter().any(|c| c.contains(ip)) {
            return Err(DeniedCategory::DeploymentRange);
        }
        Ok(())
    }
}

/// The deployment's deny-list CIDRs.
pub const DENY_CIDRS_ENV: &str = "ORDO_EGRESS_DENY_CIDRS";
/// Host names, IPs and CIDR ranges let through the private categories.
pub const PRIVATE_HOSTS_ENV: &str = "ORDO_EGRESS_PRIVATE_HOSTS";

/// One `ORDO_EGRESS_PRIVATE_HOSTS` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateHost {
    /// An exact host name, lower case.
    Name(String),
    /// An IP address (a /32 or /128) or a CIDR range.
    Net(Cidr),
}

impl PrivateHost {
    fn matches(&self, host: &str, ip: IpAddr) -> bool {
        match self {
            PrivateHost::Name(n) => n.eq_ignore_ascii_case(host),
            PrivateHost::Net(c) => c.contains(ip),
        }
    }
}

impl std::fmt::Display for PrivateHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrivateHost::Name(n) => f.write_str(n),
            PrivateHost::Net(c) => write!(f, "{}/{}", c.base, c.bits),
        }
    }
}

/// Metadata endpoints no private-host entry can open, whatever range holds
/// them: 169.254.169.254 (also caught as link-local), the AWS IPv6 address
/// fd00:ec2::254 (unique-local, so it needs this check) and Alibaba's
/// 100.100.100.200 (carrier-grade NAT space).
fn is_metadata(ip: IpAddr) -> bool {
    match canonical(ip) {
        IpAddr::V4(v4) => {
            v4.octets() == [169, 254, 169, 254] || v4.octets() == [100, 100, 100, 200]
        }
        IpAddr::V6(v6) => v6 == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x254),
    }
}

/// The non-configurable half. `pub` so a test can assert a category directly
/// without building a list, and so `deploying.md`'s disclosure can be
/// generated from the same function rather than retyped.
pub fn category_of(ip: IpAddr) -> Option<DeniedCategory> {
    match canonical(ip) {
        IpAddr::V4(v4) => v4_category(v4),
        IpAddr::V6(v6) => v6_category(v6),
    }
}

fn v4_category(a: Ipv4Addr) -> Option<DeniedCategory> {
    let o = a.octets();
    // 0.0.0.0/8 — "this network". On Linux, connecting to 0.0.0.0 reaches
    // localhost, which makes it a loopback in every sense that matters here.
    if o[0] == 0 {
        return Some(DeniedCategory::Loopback);
    }
    if a.is_loopback() {
        return Some(DeniedCategory::Loopback);
    }
    if a.is_link_local() {
        return Some(DeniedCategory::LinkLocal);
    }
    if a.is_private() {
        return Some(DeniedCategory::Private);
    }
    // 100.64.0.0/10 — RFC6598.
    if o[0] == 100 && (64..128).contains(&o[1]) {
        return Some(DeniedCategory::Cgnat);
    }
    if a.is_multicast() || a.is_broadcast() {
        return Some(DeniedCategory::Multicast);
    }
    // 192.0.0.0/24 IETF protocol assignments, 192.0.2.0/24 + 198.51.100.0/24
    // + 203.0.113.0/24 documentation, 198.18.0.0/15 benchmarking,
    // 240.0.0.0/4 reserved.
    if a.is_documentation()
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        || o[0] >= 240
    {
        return Some(DeniedCategory::Reserved);
    }
    None
}

fn v6_category(a: Ipv6Addr) -> Option<DeniedCategory> {
    let n = u128::from(a);
    if a.is_loopback() || a.is_unspecified() {
        return Some(DeniedCategory::Loopback);
    }
    // fe80::/10 — link-local unicast.
    if n >> 118 == 0x3fa {
        return Some(DeniedCategory::LinkLocal);
    }
    // fc00::/7 — unique-local. §7b writes fd00::/8; see the module doc on why
    // the wider, standard prefix is the one checked.
    if n >> 121 == 0x7e {
        return Some(DeniedCategory::UniqueLocal);
    }
    if a.is_multicast() {
        return Some(DeniedCategory::Multicast);
    }
    // 2001:db8::/32 documentation, 2002::/16 6to4 (an IPv4 address in
    // disguise, refused rather than unmapped), 100::/64 discard-only.
    if n >> 96 == 0x2001_0db8 || n >> 112 == 0x2002 || n >> 64 == 0x0100_0000_0000_0000 {
        return Some(DeniedCategory::Reserved);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_address_this_whole_file_is_about() {
        // §7b names it in prose. Here it is as an assertion.
        assert_eq!(
            category_of(ip("169.254.169.254")),
            Some(DeniedCategory::LinkLocal)
        );
    }

    #[test]
    fn ipv4_mapped_loopback_does_not_get_through() {
        // The bypass: `Ipv6Addr::is_loopback` says false and the socket
        // reaches 127.0.0.1 anyway.
        let mapped = ip("::ffff:127.0.0.1");
        assert!(!matches!(mapped, IpAddr::V6(v6) if v6.is_loopback()));
        assert_eq!(category_of(mapped), Some(DeniedCategory::Loopback));
    }

    #[test]
    fn ipv4_mapped_metadata_does_not_get_through() {
        assert_eq!(
            category_of(ip("::ffff:169.254.169.254")),
            Some(DeniedCategory::LinkLocal)
        );
    }

    #[test]
    fn nat64_is_unmapped_before_the_check() {
        // 64:ff9b::7f00:1 is 127.0.0.1 behind a NAT64 gateway.
        assert_eq!(
            category_of(ip("64:ff9b::7f00:1")),
            Some(DeniedCategory::Loopback)
        );
        // and 64:ff9b::a9fe:a9fe is the metadata endpoint.
        assert_eq!(
            category_of(ip("64:ff9b::a9fe:a9fe")),
            Some(DeniedCategory::LinkLocal)
        );
    }

    #[test]
    fn zero_page_reaches_localhost_on_linux() {
        assert_eq!(category_of(ip("0.0.0.0")), Some(DeniedCategory::Loopback));
        assert_eq!(category_of(ip("0.1.2.3")), Some(DeniedCategory::Loopback));
    }

    #[test]
    fn the_categories_seven_b_names() {
        for (a, c) in [
            ("127.0.0.1", DeniedCategory::Loopback),
            ("::1", DeniedCategory::Loopback),
            ("169.254.0.1", DeniedCategory::LinkLocal),
            ("fe80::1", DeniedCategory::LinkLocal),
            ("fd00::1", DeniedCategory::UniqueLocal),
            ("10.0.0.1", DeniedCategory::Private),
            ("172.16.0.1", DeniedCategory::Private),
            ("172.31.255.255", DeniedCategory::Private),
            ("192.168.1.1", DeniedCategory::Private),
            ("100.64.0.1", DeniedCategory::Cgnat),
            ("100.127.255.255", DeniedCategory::Cgnat),
        ] {
            assert_eq!(category_of(ip(a)), Some(c), "{a}");
        }
    }

    #[test]
    fn fc00_slash_seven_not_just_fd00_slash_eight() {
        // The widening recorded in the module doc: the document says
        // fd00::/8, the standard says fc00::/7, and the half the document
        // omits is still inside somebody's cluster.
        assert_eq!(
            category_of(ip("fc00::1")),
            Some(DeniedCategory::UniqueLocal)
        );
    }

    #[test]
    fn public_addresses_pass() {
        for a in ["1.1.1.1", "8.8.8.8", "93.184.216.34", "2606:4700::1111"] {
            assert_eq!(category_of(ip(a)), None, "{a}");
        }
    }

    #[test]
    fn a_deployment_cidr_is_the_configurable_half() {
        let deny = DenyList::categories_only()
            .with_deployment_cidrs(&DenyList::parse_cidrs("100.96.0.0/11").unwrap());
        // 100.96.0.0/11 sits inside CGNAT space, so this address is already
        // denied by category; pick one that is not.
        let deny = deny.with_deployment_cidrs(&DenyList::parse_cidrs("13.37.0.0/16").unwrap());
        assert_eq!(
            deny.check(ip("13.37.1.1")),
            Err(DeniedCategory::DeploymentRange)
        );
        assert_eq!(deny.check(ip("13.38.1.1")), Ok(()));
    }

    #[test]
    fn a_named_private_host_passes_the_private_categories_and_never_metadata() {
        let hosts = DenyList::parse_private_hosts(" Mailpit.local , localhost,").unwrap();
        assert_eq!(
            hosts,
            vec![
                PrivateHost::Name("mailpit.local".into()),
                PrivateHost::Name("localhost".into())
            ]
        );
        let deny = DenyList::categories_only()
            .with_deployment_cidrs(&DenyList::parse_cidrs("13.37.0.0/16").unwrap())
            .with_private_hosts(&hosts);
        for a in [
            "127.0.0.1",
            "::1",
            "10.1.2.3",
            "192.168.0.9",
            "fd00::1",
            "13.37.1.1",
        ] {
            assert_eq!(deny.check_host("localhost", ip(a)), Ok(()), "{a}");
            assert!(deny.check_host("relay.internal", ip(a)).is_err(), "{a}");
        }
        assert_eq!(deny.check_host("MAILPIT.local", ip("10.0.0.1")), Ok(()));
        for a in ["169.254.169.254", "fe80::1", "224.0.0.1", "192.0.2.1"] {
            assert!(deny.check_host("localhost", ip(a)).is_err(), "{a}");
        }
        assert_eq!(
            DenyList::parse_private_hosts("").unwrap(),
            Vec::<PrivateHost>::new()
        );
        for bad in [
            "*.internal",
            "localhost:1025",
            "smtp://relay",
            "relay/x",
            "10.1.0.0/8",
            "10.0.0.0/33",
            "[::1]:25",
            "-a.b",
        ] {
            assert!(DenyList::parse_private_hosts(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_ip_literal_and_a_cidr_let_an_address_through() {
        let hosts = DenyList::parse_private_hosts("10.0.0.5, 192.168.0.0/16, fd12::/16").unwrap();
        let deny = DenyList::categories_only().with_private_hosts(&hosts);
        assert_eq!(deny.check_host("10.0.0.5", ip("10.0.0.5")), Ok(()));
        assert!(deny.check_host("10.0.0.6", ip("10.0.0.6")).is_err());
        assert_eq!(deny.check_host("relay", ip("192.168.7.7")), Ok(()));
        assert_eq!(deny.check_host("relay", ip("::ffff:192.168.7.7")), Ok(()));
        assert!(deny.check_host("relay", ip("172.16.0.1")).is_err());
        assert_eq!(deny.check_host("relay", ip("fd12::9")), Ok(()));
        assert!(deny.check_host("relay", ip("fd13::9")).is_err());
    }

    #[test]
    fn metadata_stays_refused_inside_a_listed_range() {
        let hosts = DenyList::parse_private_hosts(
            "169.254.0.0/16, fd00::/8, 100.64.0.0/10, fe80::/10, 169.254.169.254, fd00:ec2::254",
        )
        .unwrap();
        let deny = DenyList::categories_only().with_private_hosts(&hosts);
        for a in [
            "169.254.169.254",
            "fd00:ec2::254",
            "::ffff:169.254.169.254",
            "100.100.100.200",
            "fe80::1",
        ] {
            assert!(deny.check_host("169.254.169.254", ip(a)).is_err(), "{a}");
        }
        assert_eq!(deny.check_host("x", ip("fd00::1")), Ok(()));
    }

    /// A tenant's provider may be on a private address, and never on a
    /// metadata address, link-local, loopback or the deployment's ranges,
    /// whatever name resolved to it.
    #[test]
    fn a_tenant_provider_reaches_private_ranges_and_never_metadata() {
        let hosts = DenyList::parse_private_hosts("idp.internal").unwrap();
        let deny = DenyList::categories_only()
            .with_deployment_cidrs(&DenyList::parse_cidrs("10.96.0.0/12").unwrap())
            .with_private_hosts(&hosts)
            .for_tenant_providers();
        for a in [
            "10.0.0.5",
            "192.168.7.7",
            "172.16.0.1",
            "fd12::9",
            "100.64.0.9",
        ] {
            assert_eq!(deny.check_host("sso.acme.example", ip(a)), Ok(()), "{a}");
        }
        assert_eq!(deny.check_host("sso.acme.example", ip("8.8.8.8")), Ok(()));
        for a in [
            "169.254.169.254",
            "::ffff:169.254.169.254",
            "fd00:ec2::254",
            "100.100.100.200",
            "169.254.1.1",
            "fe80::1",
            "127.0.0.1",
            "::1",
            "224.0.0.1",
            "10.96.0.1",
        ] {
            assert!(
                deny.check_host("metadata.example", ip(a)).is_err(),
                "{a} is refused"
            );
        }
        // The deployment's own private-host list does not carry over.
        assert!(deny.check_host("idp.internal", ip("127.0.0.1")).is_err());
        // The deployment's list is unchanged by deriving the tenant's.
        let own = DenyList::categories_only();
        assert!(own.check_host("x", ip("10.0.0.5")).is_err());
    }

    #[test]
    fn a_deployment_cidr_catches_the_mapped_form_too() {
        let deny = DenyList::categories_only()
            .with_deployment_cidrs(&DenyList::parse_cidrs("13.37.0.0/16").unwrap());
        assert_eq!(
            deny.check(ip("::ffff:13.37.1.1")),
            Err(DeniedCategory::DeploymentRange)
        );
    }

    #[test]
    fn a_cidr_with_host_bits_is_a_typo_and_is_refused() {
        assert!(Cidr::parse("10.1.0.0/8").is_err());
        assert!(Cidr::parse("10.0.0.0/8").is_ok());
        assert!(Cidr::parse("10.0.0.0/33").is_err());
        assert!(Cidr::parse("10.0.0.0").is_err());
    }
}
