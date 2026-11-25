//! Sockets from `/proc/net/{tcp,tcp6,udp,udp6}`: the files `netstat` and
//! `ss` read, readable by any user (unlike the owning process, which needs
//! root for other users' sockets).

use super::ParseError;

/// TCP `LISTEN` state.
pub const TCP_LISTEN: u8 = 0x0A;
/// UDP sockets have no listening state: a bound, unconnected socket is in
/// `TCP_CLOSE`, which is what a UDP server waiting for datagrams looks like.
pub const UDP_UNCONNECTED: u8 = 0x07;

/// One line: what is needed to tell whether a port is being listened on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Socket {
    /// Local port.
    pub port: u16,
    /// Bound to every local address (`0.0.0.0` or `::`).
    pub wildcard: bool,
    /// Kernel socket state (`st` column).
    pub state: u8,
}

/// Every socket of one file. `source` names the file in errors.
///
/// Columns: `sl local_address rem_address st …`, where an address is
/// `<hex ip>:<hex port>` (8 hex digits for IPv4, 32 for IPv6) and `st` is
/// 2 hex digits. The IP is printed in host byte order per 32-bit word, so
/// only "all zeros" (the wildcard) is read from it: that test does not
/// depend on endianness.
///
/// # Errors
///
/// No header line, or a line with too few or malformed columns.
pub fn parse(input: &str, source: &'static str) -> Result<Vec<Socket>, ParseError> {
    let mut lines = input.lines();
    if !lines
        .next()
        .is_some_and(|header| header.trim_start().starts_with("sl"))
    {
        return Err(ParseError::new(source, "no header line"));
    }
    lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let bad = || ParseError::new(source, format!("bad line '{}'", line.trim()));
            let mut fields = line.split_ascii_whitespace();
            let (Some(_), Some(local), Some(_), Some(state)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(bad());
            };
            let (ip, port) = local.split_once(':').ok_or_else(bad)?;
            if !matches!(ip.len(), 8 | 32) || !is_hex(ip) || port.len() != 4 || !is_hex(port) {
                return Err(bad());
            }
            if state.len() != 2 || !is_hex(state) {
                return Err(bad());
            }
            Ok(Socket {
                port: u16::from_str_radix(port, 16).map_err(|_| bad())?,
                wildcard: ip.bytes().all(|b| b == b'0'),
                state: u8::from_str_radix(state, 16).map_err(|_| bad())?,
            })
        })
        .collect()
}

/// `/proc/sys/net/ipv6/bindv6only`: `true` when IPv6 sockets are IPv6-only
/// by default (any non-zero integer, as the kernel reads it).
///
/// # Errors
///
/// Not a plain non-negative integer.
pub fn parse_bindv6only(input: &str) -> Result<bool, ParseError> {
    let value = input.trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::new(
            "/proc/sys/net/ipv6/bindv6only",
            format!("not an integer: '{value}'"),
        ));
    }
    // All digits: only a run of zeros is 0, however long (no overflow).
    Ok(value.bytes().any(|b| b != b'0'))
}

fn is_hex(field: &str) -> bool {
    field.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RPI4: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rpi4-ubuntu");

    fn read(name: &str) -> String {
        std::fs::read_to_string(format!("{RPI4}/proc/net/{name}")).unwrap()
    }

    #[test]
    fn rpi4_ubuntu_tcp() {
        let sockets = parse(&read("tcp"), "tcp").unwrap();
        assert_eq!(sockets.len(), 13);
        // 127.0.0.1:10010 listening.
        assert_eq!(
            sockets[0],
            Socket {
                port: 10010,
                wildcard: false,
                state: TCP_LISTEN
            }
        );
        // 0.0.0.0:22 listening, then two established ssh connections.
        assert_eq!(
            sockets[10],
            Socket {
                port: 22,
                wildcard: true,
                state: TCP_LISTEN
            }
        );
        assert_eq!(sockets[11].state, 0x01);
    }

    #[test]
    fn rpi4_ubuntu_tcp6_and_udp() {
        let tcp6 = parse(&read("tcp6"), "tcp6").unwrap();
        // :::6443, the k3s API: a dual-stack socket, absent from `tcp`.
        assert_eq!(
            tcp6[1],
            Socket {
                port: 6443,
                wildcard: true,
                state: TCP_LISTEN
            }
        );
        // ::ffff:127.0.0.1 is not the wildcard.
        assert!(!tcp6[3].wildcard);

        let udp = parse(&read("udp"), "udp").unwrap();
        assert_eq!(udp.len(), 5);
        assert!(udp.iter().all(|s| s.state == UDP_UNCONNECTED));
        let udp6 = parse(&read("udp6"), "udp6").unwrap();
        assert_eq!(
            udp6,
            [Socket {
                port: 323,
                wildcard: false,
                state: UDP_UNCONNECTED
            }]
        );
    }

    #[test]
    fn header_only_is_empty() {
        let container = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/container");
        let text = std::fs::read_to_string(format!("{container}/proc/net/tcp")).unwrap();
        assert_eq!(parse(&text, "tcp").unwrap(), []);
    }

    #[test]
    fn bindv6only() {
        assert_eq!(parse_bindv6only("0\n"), Ok(false));
        assert_eq!(parse_bindv6only("1\n"), Ok(true));
        assert_eq!(parse_bindv6only("2"), Ok(true));
        assert_eq!(parse_bindv6only("00"), Ok(false));
        for bad in ["", "\n", "-1", "yes", "1 0", "0x1"] {
            assert!(parse_bindv6only(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn malformed_input() {
        let header = "  sl  local_address rem_address   st\n";
        for body in [
            "   0: 0100007F:0016 00000000:0000\n",            // no state
            "   0: 0100007F 00000000:0000 0A\n",              // no port
            "   0: 0100007:0016 00000000:0000 0A\n",          // 7-digit address
            "   0: 0100007F:16 00000000:0000 0A\n",           // short port
            "   0: 0100007F:00G6 00000000:0000 0A\n",         // not hex
            "   0: 0100007F:0016 00000000:0000 A\n",          // 1-digit state
            "   0: 0100007F:+016 00000000:0000 0A\n",         // sign
            "   0: 0100007F:0016 00000000:0000 0A\n   1: 01", // truncated
        ] {
            assert!(
                parse(&format!("{header}{body}"), "tcp").is_err(),
                "{body:?}"
            );
        }
        assert!(parse("", "tcp").is_err(), "empty");
        assert!(
            parse("   0: 0100007F:0016 00000000:0000 0A\n", "tcp").is_err(),
            "no header"
        );
    }
}
