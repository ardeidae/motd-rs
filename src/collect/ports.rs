//! Watched ports: is something listening on them?

use std::io;

use crate::collect::{CollectError, Collector, Ctx};
use crate::config::{self, Proto, WatchedPort};
use crate::model::{Item, SectionId, ServiceState, Value};
use crate::parse::net_sockets::{self, Socket, TCP_LISTEN, UDP_UNCONNECTED};

/// The `ports` section: one row per watched port, in the config's order,
/// labelled like the config entry (`22/tcp`), valued `listening` or
/// `closed` (`?` when its socket table is unreadable). No row when nothing
/// is watched.
#[derive(Debug)]
pub struct Ports {
    watch: Vec<WatchedPort>,
}

impl Ports {
    /// Watches the ports of `[ports]`.
    #[must_use]
    pub fn new(config: &config::Ports) -> Self {
        Self {
            watch: config.watch.clone(),
        }
    }
}

impl Collector for Ports {
    fn id(&self) -> SectionId {
        SectionId::Ports
    }

    /// Never fails as a whole: an unreadable table only turns the rows that
    /// need it into `?`. Each table is read at most once.
    fn collect(&self, ctx: &Ctx) -> Result<Vec<Item>, CollectError> {
        let mut tables = Tables::default();
        Ok(self
            .watch
            .iter()
            .map(|watched| Item::new(watched.to_string(), tables.state(ctx, *watched)))
            .collect())
    }
}

/// One socket table, as read.
#[derive(Debug)]
enum Table {
    /// The IPv6 table of a kernel without IPv6 (`ipv6.disable=1`): nothing
    /// can listen there.
    Absent,
    /// Unreadable or malformed.
    Unknown,
    Sockets(Vec<Socket>),
}

/// What one table says about one port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Listening,
    NotListening,
    Unknown,
}

#[derive(Debug, Default)]
struct Tables {
    tcp: Option<Table>,
    tcp6: Option<Table>,
    udp: Option<Table>,
    udp6: Option<Table>,
    /// `net.ipv6.bindv6only`, read once and only when needed.
    v6only_default: Option<bool>,
}

impl Tables {
    fn get(&mut self, ctx: &Ctx, proto: Proto) -> &Table {
        let slot = match proto {
            Proto::Tcp => &mut self.tcp,
            Proto::Tcp6 => &mut self.tcp6,
            Proto::Udp => &mut self.udp,
            Proto::Udp6 => &mut self.udp6,
        };
        slot.get_or_insert_with(|| load(ctx, proto))
    }

    /// Whether IPv6 sockets are IPv6-only unless the program says otherwise
    /// (`net.ipv6.bindv6only = 1`). Unreadable or malformed: the kernel's
    /// default, `false`.
    fn v6only_default(&mut self, ctx: &Ctx) -> bool {
        *self.v6only_default.get_or_insert_with(|| {
            ctx.sysroot
                .read_small::<64>(BINDV6ONLY)
                .ok()
                .and_then(|text| net_sockets::parse_bindv6only(&text).ok())
                .unwrap_or(false)
        })
    }

    /// An IPv4 port also counts as listening when an IPv6 socket listens on
    /// `::` for it: such a socket is dual-stack (the Linux default,
    /// `net.ipv6.bindv6only = 0`) and accepts IPv4 clients too, yet the kernel
    /// lists it in the IPv6 table only. Go and Java servers, and k3s's API,
    /// listen this way. A socket made IPv6-only on purpose cannot be told
    /// apart from these files: that rare case is reported as listening.
    ///
    /// When the administrator made IPv6-only the default
    /// (`net.ipv6.bindv6only = 1`), a socket on `::` is taken as IPv6-only:
    /// the rule is off. A program can still opt back into dual-stack (Go
    /// does), and such a port then shows as closed over IPv4: the setting is
    /// the best hint these files give.
    ///
    /// The IPv6 table is read only when the IPv4 one has no listener: the
    /// kernel walks its whole connection hash table (sized from the RAM) to
    /// print a TCP table, about 1 ms on a desktop, whatever the number of
    /// sockets.
    fn state(&mut self, ctx: &Ctx, watched: WatchedPort) -> Value {
        let port = watched.port;
        let own = seen(self.get(ctx, watched.proto), port, watched.proto, false);
        let dual_stack = match (own, watched.proto) {
            (Seen::Listening, _) | (_, Proto::Tcp6 | Proto::Udp6) => None,
            _ if self.v6only_default(ctx) => None,
            (_, Proto::Tcp) => Some(seen(self.get(ctx, Proto::Tcp6), port, Proto::Tcp6, true)),
            (_, Proto::Udp) => Some(seen(self.get(ctx, Proto::Udp6), port, Proto::Udp6, true)),
        };
        let answers = [Some(own), dual_stack];
        if answers.contains(&Some(Seen::Listening)) {
            Value::State(ServiceState::Listening)
        } else if answers.contains(&Some(Seen::Unknown)) {
            Value::Missing
        } else {
            Value::State(ServiceState::Closed)
        }
    }
}

/// A missing IPv6 table means no IPv6. A missing IPv4 table means `/proc`
/// is not what we expect: unknown, not "closed".
fn load(ctx: &Ctx, proto: Proto) -> Table {
    let ipv6 = matches!(proto, Proto::Tcp6 | Proto::Udp6);
    match ctx
        .sysroot
        .read_to_string(&format!("/proc/net/{}", proto.as_str()))
    {
        Ok(text) => net_sockets::parse(&text, source(proto)).map_or(Table::Unknown, Table::Sockets),
        Err(err) if ipv6 && err.kind() == io::ErrorKind::NotFound => Table::Absent,
        Err(_) => Table::Unknown,
    }
}

const BINDV6ONLY: &str = "/proc/sys/net/ipv6/bindv6only";

const fn source(proto: Proto) -> &'static str {
    match proto {
        Proto::Tcp => "/proc/net/tcp",
        Proto::Tcp6 => "/proc/net/tcp6",
        Proto::Udp => "/proc/net/udp",
        Proto::Udp6 => "/proc/net/udp6",
    }
}

/// Any local address counts (a port bound to `127.0.0.1` is listening, if
/// only locally), unless `wildcard_only`.
fn seen(table: &Table, port: u16, proto: Proto, wildcard_only: bool) -> Seen {
    let listening_state = match proto {
        Proto::Tcp | Proto::Tcp6 => TCP_LISTEN,
        Proto::Udp | Proto::Udp6 => UDP_UNCONNECTED,
    };
    match table {
        Table::Absent => Seen::NotListening,
        Table::Unknown => Seen::Unknown,
        Table::Sockets(sockets) => {
            if sockets.iter().any(|s| {
                s.port == port && s.state == listening_state && (s.wildcard || !wildcard_only)
            }) {
                Seen::Listening
            } else {
                Seen::NotListening
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use crate::sysroot::Sysroot;
    use std::path::{Path, PathBuf};

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

    fn ports(entries: &[&str]) -> Ports {
        Ports {
            watch: entries
                .iter()
                .map(|e| WatchedPort::try_from((*e).to_owned()).unwrap())
                .collect(),
        }
    }

    fn rows(items: &[Item]) -> Vec<(&str, &str)> {
        items
            .iter()
            .map(|i| {
                let value = match &i.value {
                    Value::State(state) => state.as_str(),
                    Value::Missing => "?",
                    other => panic!("{other:?}"),
                };
                (i.label.as_str(), value)
            })
            .collect()
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// A temporary root holding `files` under `proc/net/` (or under `proc/`
    /// for a path with a `/`).
    fn collect_in(name: &str, watch: &[&str], files: &[(&str, &str)]) -> Vec<Item> {
        let dir = std::env::temp_dir().join(format!("motd-rs-ports-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proc/net")).unwrap();
        for (file, content) in files {
            let path = if file.contains('/') {
                dir.join("proc").join(file)
            } else {
                dir.join("proc/net").join(file)
            };
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let items = ports(watch).collect(&ctx(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        items
    }

    const V4_HEADER: &str = "  sl  local_address rem_address   st\n";
    const V6_HEADER: &str = "  sl  local_address                         remote_address  st\n";

    #[test]
    fn rpi4_ubuntu() {
        let items = ports(&[
            "22/tcp",
            "22/tcp6",
            "6443/tcp",
            "6443/tcp6",
            "10248/tcp",
            "10248/tcp6",
            "80/tcp",
            "53/udp",
            "8472/udp",
            "323/udp6",
            "68/udp6",
        ])
        .collect(&ctx(fixture("rpi4-ubuntu")))
        .unwrap();
        assert_eq!(
            rows(&items),
            [
                ("22/tcp", "listening"),
                ("22/tcp6", "listening"),
                // Only in tcp6, on `::`: dual-stack, so IPv4 clients reach it.
                ("6443/tcp", "listening"),
                ("6443/tcp6", "listening"),
                // 127.0.0.1 only: listening, locally.
                ("10248/tcp", "listening"),
                ("10248/tcp6", "closed"),
                ("80/tcp", "closed"),
                ("53/udp", "listening"),
                ("8472/udp", "listening"),
                ("323/udp6", "listening"),
                ("68/udp6", "closed"),
            ]
        );
    }

    #[test]
    fn nothing_watched_reads_nothing() {
        let items = ports(&[]).collect(&ctx("/nonexistent")).unwrap();
        assert_eq!(items, []);
    }

    #[test]
    fn established_connections_and_connected_udp_are_not_listeners() {
        let items = collect_in(
            "states",
            &["22/tcp", "53/udp"],
            &[
                // 0.0.0.0:22 established (a client's side), 0.0.0.0:53 connected UDP.
                (
                    "tcp",
                    &format!("{V4_HEADER}   0: 00000000:0016 0100007F:9C40 01\n"),
                ),
                (
                    "udp",
                    &format!("{V4_HEADER}   0: 00000000:0035 0100007F:0035 01\n"),
                ),
                ("tcp6", V6_HEADER),
                ("udp6", V6_HEADER),
            ],
        );
        assert_eq!(rows(&items), [("22/tcp", "closed"), ("53/udp", "closed")]);
    }

    #[test]
    fn dual_stack_needs_the_ipv6_wildcard() {
        // ::1:22 is IPv6 only: no IPv4 client reaches it.
        let items = collect_in(
            "loopback6",
            &["22/tcp", "22/tcp6"],
            &[
                ("tcp", V4_HEADER),
                (
                    "tcp6",
                    &format!(
                        "{V6_HEADER}   0: 00000000000000000000000001000000:0016 00000000000000000000000000000000:0000 0A\n"
                    ),
                ),
            ],
        );
        assert_eq!(
            rows(&items),
            [("22/tcp", "closed"), ("22/tcp6", "listening")]
        );
    }

    #[test]
    fn bindv6only_turns_the_dual_stack_rule_off() {
        let wildcard_22 = format!(
            "{V6_HEADER}   0: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A\n"
        );
        let run = |name, bindv6only: Option<&str>| {
            let mut files = vec![("tcp", V4_HEADER), ("tcp6", wildcard_22.as_str())];
            if let Some(value) = bindv6only {
                files.push(("sys/net/ipv6/bindv6only", value));
            }
            let items = collect_in(name, &["22/tcp", "22/tcp6"], &files);
            rows(&items)
                .into_iter()
                .map(|(label, state)| (label.to_owned(), state.to_owned()))
                .collect::<Vec<_>>()
        };
        let rows = |tcp: &str| {
            vec![
                ("22/tcp".to_owned(), tcp.to_owned()),
                ("22/tcp6".to_owned(), "listening".to_owned()),
            ]
        };
        assert_eq!(run("v6only-1", Some("1\n")), rows("closed"));
        assert_eq!(run("v6only-0", Some("0\n")), rows("listening"));
        // Unreadable or odd: the kernel default (dual-stack).
        assert_eq!(run("v6only-none", None), rows("listening"));
        assert_eq!(run("v6only-bad", Some("yes\n")), rows("listening"));
    }

    #[test]
    fn kernel_without_ipv6() {
        let items = collect_in(
            "noipv6",
            &["22/tcp", "22/tcp6", "53/udp6"],
            &[(
                "tcp",
                &format!("{V4_HEADER}   0: 00000000:0016 00000000:0000 0A\n"),
            )],
        );
        assert_eq!(
            rows(&items),
            [
                ("22/tcp", "listening"),
                ("22/tcp6", "closed"),
                ("53/udp6", "closed")
            ]
        );
    }

    #[test]
    fn unreadable_tables_only_affect_their_rows() {
        let items = collect_in(
            "broken",
            &["22/tcp", "80/tcp", "22/tcp6", "53/udp"],
            &[
                ("tcp", "garbage"),
                (
                    "tcp6",
                    &format!(
                        "{V6_HEADER}   0: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A\n"
                    ),
                ),
                // udp is missing: not "no IPv4", /proc is broken.
                ("udp6", V6_HEADER),
            ],
        );
        assert_eq!(
            rows(&items),
            [
                // Listening on `::` answers even though tcp is unreadable.
                ("22/tcp", "listening"),
                ("80/tcp", "?"),
                ("22/tcp6", "listening"),
                ("53/udp", "?"),
            ]
        );
    }
}
