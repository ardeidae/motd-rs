//! Default routes from `/proc/net/route` (IPv4) and `/proc/net/ipv6_route`
//! (IPv6). Only the default route matters here: its interface is the one
//! the machine uses to reach the outside, so its address is the one to show.

use super::ParseError;

const SOURCE_V4: &str = "/proc/net/route";
const SOURCE_V6: &str = "/proc/net/ipv6_route";

/// `RTF_UP`: the route is usable.
const RTF_UP: u32 = 0x0001;
/// `RTF_REJECT`: an "unreachable" route. Without IPv6 connectivity the kernel
/// keeps a rejecting `::/0` route on `lo`: it is not a default route.
const RTF_REJECT: u32 = 0x0200;

/// The default route of one address family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultRoute {
    /// Outgoing interface (`eth0`).
    pub interface: String,
    /// Route metric: the lowest one wins when there are several.
    pub metric: u32,
}

/// The IPv4 default route with the lowest metric (the first one on a tie,
/// as the kernel lists it). `None` without one.
///
/// Columns: `Iface Destination Gateway Flags RefCnt Use Metric Mask …`, with
/// destination, flags and mask in hex and the metric in decimal.
///
/// # Errors
///
/// No header line, or a line with too few or malformed columns.
pub fn default_v4(input: &str) -> Result<Option<DefaultRoute>, ParseError> {
    let mut lines = input.lines();
    if !lines
        .next()
        .is_some_and(|header| header.starts_with("Iface"))
    {
        return Err(ParseError::new(SOURCE_V4, "no header line"));
    }
    let mut routes = Vec::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let bad = || ParseError::new(SOURCE_V4, format!("bad line '{line}'"));
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        let field = |i: usize| fields.get(i).copied().ok_or_else(bad);
        let destination = hex(field(1)?).ok_or_else(bad)?;
        let flags = hex(field(3)?).ok_or_else(bad)?;
        let metric = field(6)?.parse::<u32>().map_err(|_| bad())?;
        let mask = hex(field(7)?).ok_or_else(bad)?;
        if destination == 0 && mask == 0 && usable(flags) {
            routes.push(DefaultRoute {
                interface: field(0)?.to_owned(),
                metric,
            });
        }
    }
    Ok(best(routes))
}

/// The IPv6 default route (`::/0`) with the lowest metric. `None` without
/// one.
///
/// Columns, all hex: destination, prefix length, source, source prefix
/// length, next hop, metric, reference count, use count, flags, interface.
///
/// # Errors
///
/// A line with too few or malformed columns.
pub fn default_v6(input: &str) -> Result<Option<DefaultRoute>, ParseError> {
    let mut routes = Vec::new();
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let bad = || ParseError::new(SOURCE_V6, format!("bad line '{line}'"));
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        let &[
            destination,
            prefix_len,
            _,
            _,
            _,
            metric,
            _,
            _,
            flags,
            interface,
        ] = fields.as_slice()
        else {
            return Err(bad());
        };
        if destination.len() != 32 || !destination.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let prefix_len = hex(prefix_len).ok_or_else(bad)?;
        let metric = hex(metric).ok_or_else(bad)?;
        let flags = hex(flags).ok_or_else(bad)?;
        let is_default = prefix_len == 0 && destination.bytes().all(|b| b == b'0');
        if is_default && usable(flags) && interface != "lo" {
            routes.push(DefaultRoute {
                interface: interface.to_owned(),
                metric,
            });
        }
    }
    Ok(best(routes))
}

fn usable(flags: u32) -> bool {
    flags & RTF_UP != 0 && flags & RTF_REJECT == 0
}

/// Lowest metric; `min_by_key` keeps the first of equal ones.
fn best(routes: Vec<DefaultRoute>) -> Option<DefaultRoute> {
    routes.into_iter().min_by_key(|route| route.metric)
}

/// A hex field of at most 8 digits, no prefix.
fn hex(field: &str) -> Option<u32> {
    if field.is_empty() || field.len() > 8 || !field.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(field, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(interface: &str, metric: u32) -> DefaultRoute {
        DefaultRoute {
            interface: interface.into(),
            metric,
        }
    }

    const HEADER: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n";

    #[test]
    fn v4_real_input() {
        // rpi4-ubuntu (k3s node): the default via eth0, plus pod routes.
        let input = format!(
            "{HEADER}\
eth0\t00000000\tFE0200C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
eth0\t000200C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
cni0\t006433C6\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
flannel.1\t007100CB\t007100CB\t0003\t0\t0\t0\t00FFFFFF\t0\t0\t0\n"
        );
        assert_eq!(default_v4(&input), Ok(Some(route("eth0", 100))));
    }

    #[test]
    fn v4_lowest_metric_wins() {
        // Laptop docked: wired (100) beats wifi (600), whatever the order.
        let input = format!(
            "{HEADER}\
wlan0\t00000000\t0102A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
eth1\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n"
        );
        assert_eq!(
            default_v4(&input),
            Ok(Some(route("eth0", 100))),
            "first on a tie"
        );
    }

    #[test]
    fn v4_no_default_route() {
        let input = format!("{HEADER}eth0\t000200C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n");
        assert_eq!(default_v4(&input), Ok(None));
        assert_eq!(default_v4(HEADER), Ok(None));
        // A default that is down or rejecting does not count.
        let input = format!(
            "{HEADER}\
eth0\t00000000\t0101A8C0\t0002\t0\t0\t0\t00000000\t0\t0\t0\n\
*\t00000000\t00000000\t0201\t0\t0\t0\t00000000\t0\t0\t0\n"
        );
        assert_eq!(default_v4(&input), Ok(None));
    }

    #[test]
    fn v4_malformed_input() {
        for bad in [
            String::new(),
            "eth0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\n".to_owned(),
            format!("{HEADER}eth0\t00000000\t0101A8C0\t0003\t0\t0\t0"),
            format!("{HEADER}eth0\tXYZ\t0101A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n"),
            format!("{HEADER}eth0\t00000000\t0101A8C0\t0003\t0\t0\t-1\t00000000\t0\t0\t0\n"),
            format!("{HEADER}eth0\t00000000\t0101A8C0\t123456789\t0\t0\t0\t00000000\t0\t0\t0\n"),
        ] {
            assert!(default_v4(&bad).is_err(), "{bad:?}");
        }
    }

    const ZERO: &str = "00000000000000000000000000000000";

    #[test]
    fn v6_default_route() {
        let input = format!(
            "fe800000000000000000000000000000 40 {ZERO} 00 {ZERO} 00000100 00000001 00000000 00000001     eth0\n\
{ZERO} 00 {ZERO} 00 fe800000000000000000000000000001 00000400 00000001 00000000 00450003    wlan0\n\
{ZERO} 00 {ZERO} 00 fe800000000000000000000000000001 00000064 00000001 00000000 00450003     eth0\n"
        );
        assert_eq!(default_v6(&input), Ok(Some(route("eth0", 0x64))));
    }

    #[test]
    fn v6_reject_route_on_lo_is_not_a_default() {
        // container fixture: no IPv6 connectivity.
        let input = format!(
            "{ZERO} 00 {ZERO} 00 {ZERO} ffffffff 00000001 00000000 00200200       lo\n\
00000000000000000000000000000001 80 {ZERO} 00 {ZERO} 00000000 00000002 00000000 80200001       lo\n"
        );
        assert_eq!(default_v6(&input), Ok(None));
        assert_eq!(default_v6(""), Ok(None));
    }

    #[test]
    fn v6_malformed_input() {
        for bad in [
            "garbage".to_owned(),
            format!("{ZERO} 00 {ZERO} 00 {ZERO} 00000400 00000001 00000000 00450003"),
            format!("{ZERO} 00 {ZERO} 00 {ZERO} 00000400 00000001 00000000 00450003 eth0 extra"),
            format!("0000 00 {ZERO} 00 {ZERO} 00000400 00000001 00000000 00450003 eth0"),
            format!("{ZERO} zz {ZERO} 00 {ZERO} 00000400 00000001 00000000 00450003 eth0"),
            format!("{ZERO} 00 {ZERO} 00 {ZERO} 00000400 00000001 00000000 nothex eth0"),
        ] {
            assert!(default_v6(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn fixtures_parse() {
        let read = |machine: &str, file: &str| {
            std::fs::read_to_string(format!(
                "{}/tests/fixtures/{machine}/proc/net/{file}",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap()
        };
        for machine in ["rpi4-ubuntu", "container"] {
            assert_eq!(
                default_v4(&read(machine, "route"))
                    .unwrap()
                    .unwrap()
                    .interface,
                "eth0"
            );
            assert_eq!(
                default_v6(&read(machine, "ipv6_route")),
                Ok(None),
                "{machine}"
            );
        }
    }
}
