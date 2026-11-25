//! Rendering: pure functions from collected facts to text.

use crate::model::{Host, Section, SectionId, Value, load_items};

/// Shown when a value could not be read.
pub const UNKNOWN: &str = "?";

/// `hostname  OS  kernel arch  up 19d 0h06m  Tue 24 Nov 10:26 CET`. Missing
/// parts are skipped, except the hostname, shown as `?`.
#[must_use]
pub fn host_line(host: &Host) -> String {
    let mut parts = vec![sanitize(host.hostname.as_deref().unwrap_or(UNKNOWN))];
    if let Some(os) = &host.os {
        parts.push(sanitize(os));
    }
    let kernel_arch: Vec<String> = [&host.kernel, &host.arch]
        .into_iter()
        .flatten()
        .map(|s| sanitize(s))
        .collect();
    if !kernel_arch.is_empty() {
        parts.push(kernel_arch.join(" "));
    }
    if let Some(secs) = host.uptime_secs {
        parts.push(format!("up {}", format_uptime(secs)));
    }
    if let Some(time) = &host.local_time {
        // The time zone name comes from the system's tz database.
        parts.push(sanitize(time));
    }
    parts.join("  ")
}

/// `procs 98  load 1.56 2.67 3.05 /4`. Temporary plain line until
/// the column layout exists: no colour, no alignment. `None` without a load
/// section.
#[must_use]
pub fn load_line(sections: &[Section]) -> Option<String> {
    let section = sections.iter().find(|s| s.id == SectionId::Load)?;
    if section.error.is_some() {
        return Some(format!("load {UNKNOWN}"));
    }
    let value = |label: &str| {
        section
            .items
            .iter()
            .find(|i| i.label == label)
            .map(|i| &i.value)
    };
    let procs = match value(load_items::PROCS) {
        Some(Value::Count(n)) => format!("procs {n}"),
        _ => format!("procs {UNKNOWN}"),
    };
    let load = match value(load_items::LOAD) {
        Some(Value::Load { hundredths, cpus }) => {
            let averages: Vec<String> = hundredths
                .iter()
                .map(|h| format!("{}.{:02}", h / 100, h % 100))
                .collect();
            let cpus = if *cpus == 0 {
                UNKNOWN.to_owned()
            } else {
                cpus.to_string()
            };
            format!("load {} /{cpus}", averages.join(" "))
        }
        _ => format!("load {UNKNOWN}"),
    };
    Some(format!("{procs}  {load}"))
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
    fn load_line_formats() {
        use crate::model::Item;
        let section = |items| vec![Section::new(SectionId::Load, items)];
        let full = section(vec![
            Item::new(
                "load",
                Value::Load {
                    hundredths: [156, 267, 5],
                    cpus: 4,
                },
            ),
            Item::new("procs", Value::Count(98)),
            Item::new("kernel_threads", Value::Count(144)),
        ]);
        assert_eq!(
            load_line(&full).unwrap(),
            "procs 98  load 1.56 2.67 0.05 /4"
        );
        let no_cpus = section(vec![
            Item::new(
                "load",
                Value::Load {
                    hundredths: [0, 0, 0],
                    cpus: 0,
                },
            ),
            Item::new("procs", Value::Count(3)),
        ]);
        assert_eq!(
            load_line(&no_cpus).unwrap(),
            "procs 3  load 0.00 0.00 0.00 /?"
        );
        let missing = section(vec![
            Item::new("load", Value::Missing),
            Item::new("procs", Value::Missing),
        ]);
        assert_eq!(load_line(&missing).unwrap(), "procs ?  load ?");
        let failed = vec![Section::failed(SectionId::Load, "timeout")];
        assert_eq!(load_line(&failed).unwrap(), "load ?");
        assert_eq!(load_line(&[]), None);
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
            arch: Some("aarch64".into()),
            uptime_secs: Some(1_642_000),
            local_time: Some("Tue 24 Nov 10:26 CET".into()),
        };
        assert_eq!(
            host_line(&full),
            "pi  Debian GNU/Linux 13 (trixie)  6.12.20-v8+ aarch64  up 19d 0h06m  Tue 24 Nov 10:26 CET"
        );
        let empty = Host {
            hostname: None,
            os: None,
            kernel: None,
            arch: None,
            uptime_secs: None,
            local_time: None,
        };
        assert_eq!(host_line(&empty), "?");
        let arch_only = Host {
            arch: Some("x86_64".into()),
            ..empty
        };
        assert_eq!(host_line(&arch_only), "?  x86_64");
    }
}
