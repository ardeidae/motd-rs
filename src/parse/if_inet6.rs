//! `/proc/net/if_inet6`: one line per IPv6 address, all fields hex:
//!
//! ```text
//! 20010db8000000000000000000000004 02 40 00 80     eth0
//! ^address (32 hex digits)          ^ifindex ^prefix length ^scope ^flags ^interface
//! ```
//!
//! `getifaddrs` lists IPv6 addresses too, but without their flags: it cannot
//! tell a stable address from a temporary (privacy) one, or a usable one
//! from one still being checked for duplicates. This file has them.

use std::net::Ipv6Addr;

use super::ParseError;

const SOURCE: &str = "/proc/net/if_inet6";

/// Scope of a global address (`IPV6_ADDR_ANY` in the kernel's scope field).
/// Link-local is `0x20`, host (loopback) `0x10`.
const SCOPE_GLOBAL: u32 = 0x00;

/// Address flags (`IFA_F_*`).
const IFA_F_TEMPORARY: u32 = 0x01;
const IFA_F_DADFAILED: u32 = 0x08;
const IFA_F_DEPRECATED: u32 = 0x20;
const IFA_F_TENTATIVE: u32 = 0x40;

/// One IPv6 address of an interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inet6 {
    /// The address.
    pub addr: Ipv6Addr,
    /// Scope field (`0x00` global, `0x20` link-local, `0x10` host).
    pub scope: u32,
    /// `IFA_F_*` flags.
    pub flags: u32,
}

impl Inet6 {
    /// Global, and usable now: not still being checked for duplicates
    /// (tentative), not found duplicate, not being phased out (deprecated).
    #[must_use]
    pub const fn is_usable_global(&self) -> bool {
        self.scope == SCOPE_GLOBAL
            && self.flags & (IFA_F_TENTATIVE | IFA_F_DADFAILED | IFA_F_DEPRECATED) == 0
    }

    /// A privacy address (RFC 8981): rotates every day or so, so it is a poor
    /// answer to "what is this machine's address".
    #[must_use]
    pub const fn is_temporary(&self) -> bool {
        self.flags & IFA_F_TEMPORARY != 0
    }
}

/// Every address of the interface named `interface`, in file order.
///
/// # Errors
///
/// A line with the wrong number of fields, a malformed address or a
/// non-hex field. Lines of other interfaces are checked too: a malformed
/// file is not trusted.
pub fn addresses_of(input: &str, interface: &str) -> Result<Vec<Inet6>, ParseError> {
    let mut found = Vec::new();
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let bad = || ParseError::new(SOURCE, format!("bad line '{line}'"));
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        let &[addr, _ifindex, _prefix_len, scope, flags, name] = fields.as_slice() else {
            return Err(bad());
        };
        let addr = parse_addr(addr).ok_or_else(bad)?;
        let scope = hex(scope).ok_or_else(bad)?;
        let flags = hex(flags).ok_or_else(bad)?;
        if name == interface {
            found.push(Inet6 { addr, scope, flags });
        }
    }
    Ok(found)
}

/// 32 hex digits → address.
fn parse_addr(field: &str) -> Option<Ipv6Addr> {
    if field.len() != 32 || !field.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(field, 16).ok().map(Ipv6Addr::from)
}

fn hex(field: &str) -> Option<u32> {
    if field.is_empty() || field.len() > 8 || !field.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(field, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &str = "\
00000000000000000000000000000001 01 80 10 80       lo
20010db8000100000000000000000004 02 40 00 80     eth0
20010db80001000012345678abcdef01 02 40 00 01     eth0
fe800000000000000000000000000004 02 40 20 80     eth0
fe80000000000000000000000000100f 0e 40 20 80 vethc2392ccf
";

    #[test]
    fn real_input() {
        let eth0 = addresses_of(INPUT, "eth0").unwrap();
        assert_eq!(
            eth0.iter().map(|a| a.addr.to_string()).collect::<Vec<_>>(),
            [
                "2001:db8:1::4",
                "2001:db8:1:0:1234:5678:abcd:ef01",
                "fe80::4"
            ]
        );
        assert!(eth0[0].is_usable_global() && !eth0[0].is_temporary());
        assert!(eth0[1].is_usable_global() && eth0[1].is_temporary());
        assert!(!eth0[2].is_usable_global(), "link-local");
        let lo = addresses_of(INPUT, "lo").unwrap();
        assert_eq!(lo[0].addr, Ipv6Addr::LOCALHOST);
        assert!(!lo[0].is_usable_global(), "host scope");
        assert_eq!(addresses_of(INPUT, "wlan0"), Ok(vec![]));
        assert_eq!(addresses_of("", "eth0"), Ok(vec![]));
    }

    #[test]
    fn unusable_flags() {
        for flags in [0x40, 0x08, 0x20, 0xc0] {
            let input = format!("20010db8000000000000000000000004 02 40 00 {flags:02x} eth0\n");
            let [a] = addresses_of(&input, "eth0").unwrap().try_into().unwrap();
            assert!(!a.is_usable_global(), "{flags:#x}");
        }
    }

    #[test]
    fn rpi4_ubuntu_fixture_has_no_global_address() {
        let input = std::fs::read_to_string(format!(
            "{}/tests/fixtures/rpi4-ubuntu/proc/net/if_inet6",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let eth0 = addresses_of(&input, "eth0").unwrap();
        assert_eq!(eth0.len(), 1);
        assert!(!eth0.iter().any(Inet6::is_usable_global));
    }

    #[test]
    fn malformed_input() {
        for bad in [
            "garbage",
            "20010db8000000000000000000000004 02 40 00 80",
            "20010db8000000000000000000000004 02 40 00 80 eth0 extra",
            "20010db800000000000000000000004 02 40 00 80 eth0",
            "20010db8000000000000000000000004x 02 40 00 80 eth0",
            "2001:db8::4 02 40 00 80 eth0",
            "20010db8000000000000000000000004 02 40 zz 80 eth0",
            "20010db8000000000000000000000004 02 40 00 123456789 eth0",
            // A malformed line of another interface still fails.
            "20010db8000000000000000000000004 02 40 00 80 eth0\nbad wlan0",
        ] {
            assert!(addresses_of(bad, "eth0").is_err(), "{bad:?}");
        }
    }
}
