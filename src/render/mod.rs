//! Rendering: pure functions from collected facts to text.

use crate::collect::host::Host;

/// Shown when a value could not be read.
pub const UNKNOWN: &str = "?";

/// `hostname  OS  kernel arch  up 19d 0h06m`. Missing parts are skipped,
/// except the hostname, shown as `?`.
#[must_use]
pub fn host_line(host: &Host) -> String {
    let mut parts = vec![sanitize(host.hostname.as_deref().unwrap_or(UNKNOWN))];
    if let Some(os) = &host.os {
        parts.push(sanitize(os));
    }
    match &host.kernel {
        Some(kernel) => parts.push(format!("{} {}", sanitize(kernel), host.arch)),
        None => parts.push(host.arch.to_owned()),
    }
    if let Some(secs) = host.uptime_secs {
        parts.push(format!("up {}", format_uptime(secs)));
    }
    parts.join("  ")
}

/// Compact uptime: `42m`, `5h07m`, `19d 0h06m`.
#[must_use]
pub fn format_uptime(secs: u64) -> String {
    let minutes = secs / 60 % 60;
    let hours = secs / 3600 % 24;
    let days = secs / 86_400;
    if days > 0 {
        format!("{days}d {hours}h{minutes:02}m")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

/// Replaces control characters (C0, DEL, C1, including ESC) with `?`, so that
/// system data can never inject terminal escape sequences.
// To do: charset detection, and transliteration when the output is ASCII.
#[must_use]
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_formats() {
        assert_eq!(format_uptime(0), "0m");
        assert_eq!(format_uptime(59), "0m");
        assert_eq!(format_uptime(42 * 60), "42m");
        assert_eq!(format_uptime(5 * 3600 + 7 * 60), "5h07m");
        assert_eq!(format_uptime(1_642_000), "19d 0h06m");
        assert_eq!(format_uptime(u64::MAX), "213503982334601d 7h00m");
    }

    #[test]
    fn sanitize_blocks_escapes() {
        assert_eq!(sanitize("/mnt/\u{1b}[31mred"), "/mnt/?[31mred");
        assert_eq!(sanitize("a\tb\nc\u{7f}\u{9b}"), "a?b?c??");
        assert_eq!(sanitize("/media/Données"), "/media/Données");
    }

    #[test]
    fn host_line_full_and_empty() {
        let full = Host {
            hostname: Some("pi".into()),
            os: Some("Debian GNU/Linux 13 (trixie)".into()),
            kernel: Some("6.12.20-v8+".into()),
            arch: "aarch64",
            uptime_secs: Some(1_642_000),
        };
        assert_eq!(
            host_line(&full),
            "pi  Debian GNU/Linux 13 (trixie)  6.12.20-v8+ aarch64  up 19d 0h06m"
        );
        let empty = Host {
            hostname: None,
            os: None,
            kernel: None,
            arch: "x86_64",
            uptime_secs: None,
        };
        assert_eq!(host_line(&empty), "?  x86_64");
    }
}
