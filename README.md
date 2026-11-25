# motd-rs

A fast, compact, read-only system status dashboard for the Linux message of
the day.

> **Status: early development.** It prints the host line (hostname, OS,
> kernel, architecture, uptime, date), the default-route interface with its
> IPv4 and IPv6 addresses, the process count and load, memory (RAM, swap,
> cache, buffers) and disk usage on plain lines. Users, services, ports and
> the column layout are coming.

## Goals

- **Fast**: a few milliseconds, no subprocesses. It reads `/proc`, `/sys` and systemd over D-Bus.
- **Light**: a single static binary with very few dependencies.
- **Compact**: about ten lines, no boxes, colour only where it matters.
- **Clean in every terminal**: ASCII by default, Nerd Font icons as an option.
- **Never blocks a login**: a strict time limit, and every section can fail on its own without affecting the others.
- **Wide support**: Debian, Ubuntu, Raspberry Pi OS, RHEL and its rebuilds (8+), Fedora.

## Install from source

```sh
cargo install motd-rs
motd-rs
```

## License

MIT, see [LICENSE](LICENSE).
