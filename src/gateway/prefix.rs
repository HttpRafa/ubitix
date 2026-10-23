use std::net::Ipv6Addr;

use color_eyre::eyre::{Result, eyre};
use ipnet::Ipv6Net;
use tokio::fs;

/// A prefix or address learned from the kernel's IPv6 tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPrefix {
    pub addr: Ipv6Addr,
    pub prefix_len: u8,
    pub iface: String,
}

/// True for prefixes this daemon may install NAT rules for.
///
/// Link-local, multicast, loopback, and unspecified prefixes are always
/// present or meaningless as delegated prefixes. Accepting them from syslog
/// would let a local user rewrite neighbor-discovery or loopback traffic.
pub fn acceptable_delegated_prefix(prefix: &Ipv6Net) -> bool {
    if prefix.prefix_len() > 64 {
        return false;
    }
    let addr = prefix.network();
    if addr.is_unspecified()
        || addr.is_loopback()
        || addr.is_multicast()
        || addr.is_unicast_link_local()
        || addr.to_ipv4_mapped().is_some()
    {
        return false;
    }
    true
}

pub fn parse_ipv6_route(text: &str) -> Vec<InstalledPrefix> {
    let mut prefixes = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(dest), Some(prefix_len)) = (fields.next(), fields.next()) else {
            continue;
        };
        // src, src_len, gateway, metric, refcnt, use, flags, then the device.
        let rest: Vec<_> = fields.collect();
        if rest.len() < 8 {
            continue;
        }
        let Some(iface) = rest.last() else {
            continue;
        };
        let (Some(addr), Some(prefix_len)) = (parse_ipv6_hex(dest), parse_hex_u8(prefix_len))
        else {
            continue;
        };
        if prefix_len > 128 {
            continue;
        }
        prefixes.push(InstalledPrefix {
            addr,
            prefix_len,
            iface: (*iface).to_string(),
        });
    }
    prefixes
}

pub fn parse_if_inet6(text: &str) -> Vec<InstalledPrefix> {
    let mut prefixes = Vec::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        let (Some(addr), Some(prefix_len)) = (parse_ipv6_hex(fields[0]), parse_hex_u8(fields[2]))
        else {
            continue;
        };
        if prefix_len > 128 {
            continue;
        }
        prefixes.push(InstalledPrefix {
            addr,
            prefix_len,
            iface: fields[5].to_string(),
        });
    }
    prefixes
}

/// A log line is accepted only when the kernel already has that exact prefix.
///
/// Matching a shorter route that merely contains some local /64 would let an
/// unprivileged `logger` user select any supernet of a LAN prefix. Routes on
/// `lo` count because delegated prefixes are often installed as unreachable
/// routes there; addresses on `lo` do not.
pub fn kernel_confirms(
    claim: &Ipv6Net,
    iface: &str,
    routes: &[InstalledPrefix],
    addresses: &[InstalledPrefix],
) -> bool {
    let claim = claim.trunc();
    routes
        .iter()
        .any(|entry| entry_matches(&claim, iface, entry, true))
        || addresses
            .iter()
            .any(|entry| entry_matches(&claim, iface, entry, false))
}

pub async fn delegated_prefix_is_installed(prefix: &Ipv6Net, iface: &str) -> Result<bool> {
    let routes = fs::read_to_string("/proc/net/ipv6_route")
        .await
        .map_err(|error| eyre!("failed to read /proc/net/ipv6_route: {error}"))?;
    let addresses = fs::read_to_string("/proc/net/if_inet6")
        .await
        .map_err(|error| eyre!("failed to read /proc/net/if_inet6: {error}"))?;
    Ok(kernel_confirms(
        prefix,
        iface,
        &parse_ipv6_route(&routes),
        &parse_if_inet6(&addresses),
    ))
}

fn entry_matches(claim: &Ipv6Net, iface: &str, entry: &InstalledPrefix, allow_lo: bool) -> bool {
    if entry.prefix_len != claim.prefix_len() {
        return false;
    }
    if entry.iface != iface && !(allow_lo && entry.iface == "lo") {
        return false;
    }
    Ipv6Net::new(entry.addr, entry.prefix_len)
        .map(|net| net.network() == claim.network())
        .unwrap_or(false)
}

fn parse_ipv6_hex(text: &str) -> Option<Ipv6Addr> {
    if text.len() != 32 {
        return None;
    }
    let value = u128::from_str_radix(text, 16).ok()?;
    Some(Ipv6Addr::from(value))
}

fn parse_hex_u8(text: &str) -> Option<u8> {
    u8::from_str_radix(text, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(text: &str) -> Ipv6Net {
        text.parse().unwrap()
    }

    fn installed(addr: &str, prefix_len: u8, iface: &str) -> InstalledPrefix {
        InstalledPrefix {
            addr: addr.parse().unwrap(),
            prefix_len,
            iface: iface.to_string(),
        }
    }

    #[test]
    fn rejects_prefixes_that_must_not_be_rewritten() {
        assert!(acceptable_delegated_prefix(&net("2001:db8:1:200::/56")));
        assert!(acceptable_delegated_prefix(&net("fd00:1:2::/48")));
        assert!(!acceptable_delegated_prefix(&net("fe80::/64")));
        assert!(!acceptable_delegated_prefix(&net("ff00::/8")));
        assert!(!acceptable_delegated_prefix(&net("::/0")));
        assert!(!acceptable_delegated_prefix(&net("::1/128")));
        assert!(!acceptable_delegated_prefix(&net("2001:db8::/96")));
        assert!(!acceptable_delegated_prefix(&net("::ffff:0:0/64")));
    }

    #[test]
    fn parses_proc_route_and_address_lines() {
        let routes = parse_ipv6_route(
            "fe800000000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001   enp0s3\n",
        );
        assert_eq!(routes, vec![installed("fe80::", 64, "enp0s3")]);

        let addresses = parse_if_inet6("fe80000000000000802b38fffe3dbcd0 02 40 20 80   enp0s3\n");
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0].prefix_len, 64);
        assert_eq!(addresses[0].iface, "enp0s3");
        assert_eq!(
            addresses[0].addr,
            "fe80::802b:38ff:fe3d:bcd0".parse::<Ipv6Addr>().unwrap()
        );
    }

    #[test]
    fn confirmation_requires_the_same_prefix_on_the_interface_or_lo() {
        let claim = net("2001:db8:1:200::/56");
        let on_wan = installed("2001:db8:1:200::", 56, "ppp1");
        assert!(kernel_confirms(
            &claim,
            "ppp1",
            std::slice::from_ref(&on_wan),
            &[]
        ));
        assert!(!kernel_confirms(&claim, "eth0", &[on_wan], &[]));

        let more_specific = installed("2001:db8:1:200::", 64, "ppp1");
        assert!(!kernel_confirms(&claim, "ppp1", &[more_specific], &[]));

        let on_lan = installed("2001:db8:1:200::", 56, "br0");
        assert!(!kernel_confirms(
            &claim,
            "ppp1",
            std::slice::from_ref(&on_lan),
            &[]
        ));
        assert!(!kernel_confirms(&claim, "ppp1", &[], &[on_lan]));

        let on_lo = installed("2001:db8:1:200::", 56, "lo");
        assert!(kernel_confirms(
            &claim,
            "ppp1",
            std::slice::from_ref(&on_lo),
            &[]
        ));
        assert!(!kernel_confirms(&claim, "ppp1", &[], &[on_lo]));

        let address = installed("2001:db8:1:200::1", 56, "ppp1");
        assert!(kernel_confirms(&claim, "ppp1", &[], &[address]));
    }

    #[test]
    fn live_proc_tables_parse() {
        let routes = parse_ipv6_route(&std::fs::read_to_string("/proc/net/ipv6_route").unwrap());
        let addresses = parse_if_inet6(&std::fs::read_to_string("/proc/net/if_inet6").unwrap());
        assert!(routes.iter().any(|route| route.iface == "lo"));
        assert!(addresses.iter().any(|address| address.iface == "lo"));
        let random = net("2001:db8:dead:beef::/56");
        assert!(!kernel_confirms(&random, "eth9", &routes, &addresses));
    }
}
