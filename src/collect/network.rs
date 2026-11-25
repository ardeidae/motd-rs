//! Where the machine is reached from: the interface of each default route
//! and its address.

use std::net::{IpAddr, Ipv6Addr};

use crate::collect::{CollectError, Collector, Ctx};
use crate::model::{Item, SectionId, Value, net_items};
use crate::parse::if_inet6::{self, Inet6};
use crate::parse::route::{self, DefaultRoute};

/// The `net` section: an [`net_items::IPV4`] item when there is an IPv4
/// default route, an [`net_items::IPV6`] one when there is an IPv6 default
/// route, in that order. No item for a family without a default route: an
/// IPv4-only network is normal, not an error.
#[derive(Debug)]
pub struct Network;

impl Collector for Network {
    fn id(&self) -> SectionId {
        SectionId::Network
    }

    /// IPv4 is the main answer: an unreadable or malformed
    /// `/proc/net/route` fails the section. IPv6 is optional (a kernel
    /// booted with `ipv6.disable=1` has no IPv6 files): a missing file means
    /// no IPv6, and a malformed one only turns the IPv6 row into `?`.
    fn collect(&self, ctx: &Ctx) -> Result<Vec<Item>, CollectError> {
        let path = "/proc/net/route";
        let text = ctx
            .sysroot
            .read_to_string(path)
            .map_err(|source| CollectError::Io { path, source })?;
        let mut items = Vec::new();
        if let Some(DefaultRoute { interface, .. }) = route::default_v4(&text)? {
            let addr = ipv4_address(ctx, &interface).map(IpAddr::V4);
            items.push(Item::new(
                net_items::IPV4,
                Value::Address { interface, addr },
            ));
        }
        if let Some(value) = ipv6(ctx) {
            items.push(Item::new(net_items::IPV6, value));
        }
        Ok(items)
    }
}

/// The first IPv4 address of `interface`, as `getifaddrs` lists them (the
/// primary address comes first). `None` when it has none or the call fails.
fn ipv4_address(ctx: &Ctx, interface: &str) -> Option<std::net::Ipv4Addr> {
    ctx.sys
        .ipv4_interfaces()
        .ok()?
        .into_iter()
        .find(|entry| entry.name == interface)
        .map(|entry| entry.addr)
}

/// The IPv6 row, `None` without an IPv6 default route.
fn ipv6(ctx: &Ctx) -> Option<Value> {
    // No file: no IPv6 in this kernel, nothing to say.
    let Ok(routes) = ctx.sysroot.read_to_string("/proc/net/ipv6_route") else {
        return None;
    };
    let interface = match route::default_v6(&routes) {
        Ok(route) => route?.interface,
        Err(_) => return Some(Value::Missing),
    };
    let addr = ctx
        .sysroot
        .read_to_string("/proc/net/if_inet6")
        .ok()
        .and_then(|text| if_inet6::addresses_of(&text, &interface).ok())
        .and_then(|addresses| best_ipv6(&addresses))
        .map(IpAddr::V6);
    Some(Value::Address { interface, addr })
}

/// The address that best answers "what is this machine's IPv6 address":
/// usable and global, then stable before temporary (privacy addresses
/// rotate), then public (`2000::/3`) before unique-local (`fd00::/8`, only
/// reachable inside the site). File order breaks ties.
fn best_ipv6(addresses: &[Inet6]) -> Option<Ipv6Addr> {
    addresses
        .iter()
        .filter(|a| a.is_usable_global())
        .min_by_key(|a| (a.is_temporary(), !is_public(a.addr)))
        .map(|a| a.addr)
}

/// Global unicast, `2000::/3`: the addresses routed on the internet.
fn is_public(addr: Ipv6Addr) -> bool {
    addr.segments()
        .first()
        .is_some_and(|&first| first & 0xe000 == 0x2000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use crate::sysroot::Sysroot;
    use std::path::PathBuf;

    const ZERO: &str = "00000000000000000000000000000000";
    const ROUTE_HEADER: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n";

    fn ctx(root: impl Into<PathBuf>) -> Ctx {
        let sysroot = Sysroot::new(root);
        let (sys, systemd) = Fake::load(sysroot.root())
            .unwrap_or_default()
            .into_backends();
        Ctx {
            sysroot,
            sys,
            systemd,
        }
    }

    fn values(ctx: &Ctx) -> Vec<(String, Value)> {
        Network
            .collect(ctx)
            .unwrap()
            .into_iter()
            .map(|i| (i.label, i.value))
            .collect()
    }

    fn address(interface: &str, addr: Option<&str>) -> Value {
        Value::Address {
            interface: interface.into(),
            addr: addr.map(|a| a.parse().unwrap()),
        }
    }

    /// A temporary root with the given files, removed by the caller.
    fn root(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("motd-rs-net-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    fn with_root(name: &str, files: &[(&str, &str)]) -> Vec<(String, Value)> {
        let dir = root(name, files);
        let result = values(&ctx(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        result
    }

    fn fixture(name: &str) -> Vec<(String, Value)> {
        values(&ctx(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        )))
    }

    #[test]
    fn rpi4_ubuntu_is_ipv4_only() {
        assert_eq!(
            fixture("rpi4-ubuntu"),
            [("ipv4".to_owned(), address("eth0", Some("192.0.2.1")))]
        );
    }

    #[test]
    fn container_ignores_the_reject_route() {
        assert_eq!(
            fixture("container"),
            [("ipv4".to_owned(), address("eth0", Some("192.0.2.5")))]
        );
    }

    #[test]
    fn dual_stack() {
        let route =
            format!("{ROUTE_HEADER}eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n");
        let route6 = format!(
            "{ZERO} 00 {ZERO} 00 fe800000000000000000000000000001 00000400 00000001 00000000 00450003     eth0\n"
        );
        // Temporary, unique-local and stable public addresses: the stable
        // public one wins, whatever the order.
        let inet6 = "\
20010db80001000012345678abcdef01 02 40 00 01     eth0
fd000000000000000000000000000004 02 40 00 80     eth0
20010db8000100000000000000000004 02 40 00 80     eth0
fe800000000000000000000000000004 02 40 20 80     eth0
";
        let rows = with_root(
            "dual",
            &[
                ("proc/net/route", &route),
                ("proc/net/ipv6_route", &route6),
                ("proc/net/if_inet6", inet6),
            ],
        );
        assert_eq!(
            rows,
            [
                // No fake.toml: getifaddrs knows no interface.
                ("ipv4".to_owned(), address("eth0", None)),
                ("ipv6".to_owned(), address("eth0", Some("2001:db8:1::4"))),
            ]
        );
    }

    #[test]
    fn best_ipv6_preferences() {
        let a = |addr: &str, scope, flags| Inet6 {
            addr: addr.parse().unwrap(),
            scope,
            flags,
        };
        let ula = a("fd00::4", 0, 0x80);
        let temporary = a("2001:db8::99", 0, 0x01);
        let tentative = a("2001:db8::5", 0, 0x40);
        let link_local = a("fe80::4", 0x20, 0x80);
        assert_eq!(best_ipv6(&[temporary, ula]), Some(ula.addr), "stable first");
        assert_eq!(
            best_ipv6(&[temporary]),
            Some(temporary.addr),
            "better than none"
        );
        assert_eq!(best_ipv6(&[tentative, link_local]), None);
        assert_eq!(best_ipv6(&[]), None);
    }

    #[test]
    fn ipv6_problems_stay_in_the_ipv6_row() {
        let route =
            format!("{ROUTE_HEADER}eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n");
        // Malformed ipv6_route: IPv6 is "?", IPv4 still shown.
        let rows = with_root(
            "bad-v6",
            &[
                ("proc/net/route", &route),
                ("proc/net/ipv6_route", "garbage\n"),
            ],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], ("ipv6".to_owned(), Value::Missing));
        // A default route but no if_inet6: interface without address.
        let route6 =
            format!("{ZERO} 00 {ZERO} 00 {ZERO} 00000400 00000001 00000000 00000003 wg0\n");
        let rows = with_root(
            "no-inet6",
            &[("proc/net/route", &route), ("proc/net/ipv6_route", &route6)],
        );
        assert_eq!(rows[1], ("ipv6".to_owned(), address("wg0", None)));
    }

    #[test]
    fn no_default_route_no_row() {
        let rows = with_root("none", &[("proc/net/route", ROUTE_HEADER)]);
        assert_eq!(rows.len(), 0);
    }

    #[test]
    fn unreadable_or_malformed_route_fails_the_section() {
        let err = Network
            .collect(&ctx("/nonexistent-motd-rs-root"))
            .unwrap_err();
        assert_eq!(err.reason(), "unreadable");
        let dir = root("bad-v4", &[("proc/net/route", "garbage\n")]);
        let err = Network.collect(&ctx(&dir)).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(err.reason(), "unparsable");
    }
}
