//! Who a request comes from, and whether it reached Gantry over HTTPS (PRD
//! §9.1, §9.2).
//!
//! Behind a TLS-terminating proxy or tunnel, every connection comes from the
//! proxy, and only its `X-Forwarded-For` and `X-Forwarded-Proto` headers say
//! who the real client is and whether they used HTTPS. Anyone can send those
//! headers, so they are believed only from an address in `trusted_proxies`.
//! From anywhere else they are ignored.
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use axum::extract::{ConnectInfo, Request};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;

use crate::AppState;

/// One address, or a range of them in CIDR form, such as `10.0.0.0/8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRange {
    network: IpAddr,
    prefix: u8,
}

impl IpRange {
    /// Whether `ip` is in the range. An IPv4 address mapped into IPv6
    /// (`::ffff:a.b.c.d`) counts as the IPv4 address.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, canonical(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl FromStr for IpRange {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (value, None),
        };
        let network = canonical(
            address
                .parse::<IpAddr>()
                .map_err(|_| format!("`{value}` is not an IP address or a CIDR range"))?,
        );
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix =
            match prefix {
                None => max,
                Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max).ok_or_else(|| {
                    format!("`{value}` has a prefix length that isn't 0 to {max}")
                })?,
            };
        Ok(Self { network, prefix })
    }
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

/// The proxies whose forwarded headers are believed. Empty by default: then
/// forwarded headers are always ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedProxies(Vec<IpRange>);

impl TrustedProxies {
    pub fn new(ranges: Vec<IpRange>) -> Self {
        Self(ranges)
    }

    /// Parses a comma-separated list, such as `127.0.0.1, 172.16.0.0/12`.
    pub fn parse_list(value: &str) -> Result<Self, String> {
        value
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(IpRange::from_str)
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    pub fn trusts(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|range| range.contains(ip))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Marks a request that arrived on a connection Gantry's own TLS terminated.
#[derive(Debug, Clone, Copy)]
pub struct DirectTls;

/// Who sent a request, as far as Gantry can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClientInfo {
    /// The real client's address: the connection's, or, through a trusted
    /// proxy, the one it forwarded. `None` when unknown (in tests).
    pub ip: Option<IpAddr>,
    /// Whether the client's own connection is HTTPS: to Gantry directly, or
    /// to a trusted proxy that says so.
    pub https: bool,
    /// Whether it came through a trusted proxy or tunnel.
    pub via_proxy: bool,
}

impl ClientInfo {
    /// How the audit log and login limits name the client.
    pub fn label(&self) -> String {
        self.ip
            .map_or_else(|| "unknown".to_owned(), |ip| ip.to_string())
    }

    /// Whether the client is somewhere other than this network (PRD §9.3):
    /// it came through a proxy or tunnel, or its address is not a private,
    /// loopback or link-local one. A VPN such as Tailscale (100.64.0.0/10)
    /// counts as remote, since its traffic crosses the internet.
    pub fn is_remote(&self) -> bool {
        self.via_proxy || self.ip.is_some_and(|ip| !is_local(canonical(ip)))
    }
}

/// Loopback, private (RFC 1918, IPv6 unique-local) and link-local addresses.
fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6 == Ipv6Addr::UNSPECIFIED
        }
    }
}

/// Works out the client from the connection and, only when the connection
/// is from a trusted proxy, its forwarded headers.
pub fn client_info(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    direct_tls: bool,
    proxies: &TrustedProxies,
) -> ClientInfo {
    let peer_ip = peer.map(|p| canonical(p.ip()));
    let Some(peer_ip) = peer_ip.filter(|ip| proxies.trusts(*ip)) else {
        return ClientInfo {
            ip: peer_ip,
            https: direct_tls,
            via_proxy: false,
        };
    };
    // Each proxy appends the address it saw, so walking from the right, the
    // first address that isn't one of our proxies is the client. Anything to
    // its left came from the client and can't be believed.
    let forwarded: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|v| parse_forwarded_ip(v.trim()))
        .collect();
    let ip = forwarded
        .iter()
        .rev()
        .copied()
        .find(|ip| !proxies.trusts(*ip))
        .or_else(|| forwarded.first().copied())
        .unwrap_or(peer_ip);
    let https = direct_tls
        || headers
            .get_all("x-forwarded-proto")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .next_back()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("https"));
    ClientInfo {
        ip: Some(ip),
        https,
        via_proxy: true,
    }
}

/// An address from `X-Forwarded-For`, which may carry a port
/// (`203.0.113.9:4711`, `[2001:db8::1]:4711`).
fn parse_forwarded_ip(value: &str) -> Option<IpAddr> {
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(canonical(ip));
    }
    if let Ok(addr) = value.parse::<SocketAddr>() {
        return Some(canonical(addr.ip()));
    }
    value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .and_then(|v| v.parse::<Ipv6Addr>().ok())
        .map(|v6| canonical(IpAddr::V6(v6)))
}

/// Middleware: works out the [`ClientInfo`] once and puts it in the request
/// for handlers.
pub async fn identify(
    axum::extract::State(state): axum::extract::State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let direct_tls = request.extensions().get::<DirectTls>().is_some();
    let info = client_info(request.headers(), peer, direct_tls, state.trusted_proxies());
    request.extensions_mut().insert(info);
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        map
    }

    fn peer(ip: &str) -> Option<SocketAddr> {
        Some(SocketAddr::new(ip.parse().unwrap(), 5000))
    }

    #[test]
    fn ranges_parse_and_match() {
        let range: IpRange = "10.0.0.0/8".parse().unwrap();
        assert!(range.contains("10.1.2.3".parse().unwrap()));
        assert!(!range.contains("11.0.0.1".parse().unwrap()));
        assert!(range.contains("::ffff:10.0.0.9".parse().unwrap()));
        let one: IpRange = "127.0.0.1".parse().unwrap();
        assert!(one.contains("127.0.0.1".parse().unwrap()));
        assert!(!one.contains("127.0.0.2".parse().unwrap()));
        let v6: IpRange = "fd00::/8".parse().unwrap();
        assert!(v6.contains("fd12::1".parse().unwrap()));
        let all: IpRange = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains("8.8.8.8".parse().unwrap()));
        assert!("10.0.0.0/33".parse::<IpRange>().is_err());
        assert!("not-an-ip".parse::<IpRange>().is_err());
        assert_eq!(
            TrustedProxies::parse_list(" 127.0.0.1, 172.16.0.0/12 ,")
                .unwrap()
                .0
                .len(),
            2
        );
    }

    #[test]
    fn forwarded_headers_are_ignored_from_an_untrusted_peer() {
        let forged = headers(&[
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-proto", "https"),
        ]);
        let none = TrustedProxies::default();
        let info = client_info(&forged, peer("192.168.1.20"), false, &none);
        assert_eq!(info.ip, Some("192.168.1.20".parse().unwrap()));
        assert!(
            !info.https,
            "a forged X-Forwarded-Proto doesn't make it HTTPS"
        );
        assert!(!info.via_proxy);

        // Trusting some other proxy doesn't help a client that isn't it.
        let other = TrustedProxies::parse_list("10.9.9.9").unwrap();
        let info = client_info(&forged, peer("192.168.1.20"), false, &other);
        assert_eq!(info.ip, Some("192.168.1.20".parse().unwrap()));
        assert!(!info.https);
    }

    #[test]
    fn a_trusted_proxy_names_the_client_and_the_scheme() {
        let proxies = TrustedProxies::parse_list("127.0.0.1, 10.0.0.0/8").unwrap();
        let info = client_info(
            &headers(&[
                // The client claims to be 1.1.1.1; the outer proxy saw
                // 203.0.113.9 and the inner one saw the outer proxy.
                ("x-forwarded-for", "1.1.1.1, 203.0.113.9"),
                ("x-forwarded-for", "10.0.0.5"),
                ("x-forwarded-proto", "https"),
            ]),
            peer("127.0.0.1"),
            false,
            &proxies,
        );
        assert_eq!(info.ip, Some("203.0.113.9".parse().unwrap()));
        assert!(info.https);
        assert!(info.via_proxy);
        assert!(info.is_remote());

        // No header: the proxy itself is all that is known.
        let info = client_info(&HeaderMap::new(), peer("127.0.0.1"), false, &proxies);
        assert_eq!(info.ip, Some("127.0.0.1".parse().unwrap()));
        assert!(!info.https);
        // A port on the forwarded address is fine.
        let info = client_info(
            &headers(&[("x-forwarded-for", "[2001:db8::7]:4711")]),
            peer("127.0.0.1"),
            false,
            &proxies,
        );
        assert_eq!(info.ip, Some("2001:db8::7".parse().unwrap()));
    }

    #[test]
    fn direct_tls_is_https_whatever_the_headers() {
        let info = client_info(
            &HeaderMap::new(),
            peer("192.168.1.5"),
            true,
            &TrustedProxies::default(),
        );
        assert!(info.https);
    }

    #[test]
    fn remote_means_outside_the_lan_or_through_a_proxy() {
        let direct = |ip: &str| ClientInfo {
            ip: Some(ip.parse().unwrap()),
            https: false,
            via_proxy: false,
        };
        assert!(!direct("192.168.1.10").is_remote());
        assert!(!direct("10.1.1.1").is_remote());
        assert!(!direct("127.0.0.1").is_remote());
        assert!(!direct("fe80::1").is_remote());
        assert!(!direct("fd00::5").is_remote());
        assert!(direct("100.101.102.103").is_remote(), "Tailscale is remote");
        assert!(direct("203.0.113.9").is_remote());
        assert!(direct("2001:db8::1").is_remote());
        let proxied = ClientInfo {
            via_proxy: true,
            ..direct("192.168.1.10")
        };
        assert!(proxied.is_remote());
        assert!(
            !ClientInfo {
                ip: None,
                https: false,
                via_proxy: false
            }
            .is_remote()
        );
    }
}
