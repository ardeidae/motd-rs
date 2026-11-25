//! Rendering: pure functions from collected facts to text.

use crate::model::{Host, Item, Section, SectionId, Value, load_items, memory_items};

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

/// `ram 45% 3.3G/7.6G  swap none  cache 3.1G  buffers 244M`. Temporary
/// plain line until the column layout exists, like [`load_line`]. Cache and
/// buffers are informational: skipped when unreadable. `None` without a
/// memory section.
#[must_use]
pub fn memory_line(sections: &[Section]) -> Option<String> {
    let section = sections.iter().find(|s| s.id == SectionId::Memory)?;
    if section.error.is_some() {
        return Some(format!("{} {UNKNOWN}", memory_items::RAM));
    }
    let mut parts = Vec::new();
    for Item { label, value, .. } in &section.items {
        let part = match value {
            Value::Usage { total: 0, .. } if label == memory_items::SWAP => format!("{label} none"),
            Value::Usage { used, total } => format!(
                "{label} {} {}/{}",
                format_percent(*used, *total),
                format_size(*used),
                format_size(*total)
            ),
            Value::Bytes(bytes) => format!("{label} {}", format_size(*bytes)),
            Value::Missing if label == memory_items::RAM || label == memory_items::SWAP => {
                format!("{label} {UNKNOWN}")
            }
            _ => continue,
        };
        parts.push(part);
    }
    Some(parts.join("  "))
}

/// IEC size in at most 4 characters, no locale: `999B`, `1.0K`, `9.9M`,
/// `38G`, `222M`, `16E`. One decimal below 10 units, none from 10 up; a
/// value that would need 4 digits moves to the next unit (`1000M` → `1.0G`).
#[must_use]
pub fn format_size(bytes: u64) -> String {
    const UNITS: [char; 6] = ['K', 'M', 'G', 'T', 'P', 'E'];
    if bytes < 1000 {
        return format!("{bytes}B");
    }
    let bytes = u128::from(bytes);
    let mut divisor: u128 = 1;
    for unit in UNITS {
        divisor *= 1024;
        // Rounded to the nearest tenth, then to the nearest unit.
        let tenths = (bytes * 10 + divisor / 2) / divisor;
        if tenths < 100 {
            return format!("{}.{}{unit}", tenths / 10, tenths % 10);
        }
        let whole = (bytes + divisor / 2) / divisor;
        if whole < 1000 {
            return format!("{whole}{unit}");
        }
    }
    // Unreachable: u64::MAX is 16E.
    UNKNOWN.to_owned()
}

/// `used` as a whole percentage of `total`, rounded down like the
/// thresholds (`79%` is never shown for a row in warning at 80). `?` when
/// `total` is 0.
#[must_use]
pub fn format_percent(used: u64, total: u64) -> String {
    (u128::from(used) * 100)
        .checked_div(u128::from(total))
        .map_or_else(|| UNKNOWN.to_owned(), |percent| format!("{percent}%"))
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
    fn size_formats() {
        let cases: [(u64, &str); 16] = [
            (0, "0B"),
            (999, "999B"),
            (1000, "1.0K"),
            (1024, "1.0K"),
            (1536, "1.5K"),
            (10_188, "9.9K"),
            (10_189, "10K"),
            (222 * 1024 * 1024, "222M"),
            (1023 * 1024 * 1024, "1.0G"),
            (999 * 1024 * 1024, "999M"),
            (999 * 1024 * 1024 + 512 * 1024, "1.0G"),
            (38 * (1 << 30), "38G"),
            (1_288_490_189, "1.2G"),
            (3_624_796 * 1024, "3.5G"),
            (1 << 60, "1.0E"),
            (u64::MAX, "16E"),
        ];
        for (bytes, text) in cases {
            assert_eq!(format_size(bytes), text, "{bytes}");
            assert!(format_size(bytes).len() <= 4, "{bytes}");
        }
        // Every power of two and its neighbours stay within 4 characters.
        for shift in 0..64 {
            for bytes in [(1_u64 << shift) - 1, 1 << shift, (1 << shift) + 1] {
                assert!(format_size(bytes).len() <= 4, "{bytes}");
            }
        }
    }

    #[test]
    fn percent_formats() {
        assert_eq!(format_percent(0, 10), "0%");
        assert_eq!(format_percent(7999, 10_000), "79%");
        assert_eq!(format_percent(10, 10), "100%");
        assert_eq!(format_percent(u64::MAX, u64::MAX), "100%");
        assert_eq!(format_percent(1, 0), "?");
    }

    #[test]
    fn memory_line_formats() {
        let section = |items| vec![Section::new(SectionId::Memory, items)];
        let gib = 1 << 30;
        let full = section(vec![
            Item::new(
                "ram",
                Value::Usage {
                    used: gib,
                    total: 4 * gib,
                },
            ),
            Item::new(
                "swap",
                Value::Usage {
                    used: 1 << 20,
                    total: 100 << 20,
                },
            ),
            Item::new("cache", Value::Bytes(gib)),
            Item::new("buffers", Value::Bytes(28 << 20)),
        ]);
        assert_eq!(
            memory_line(&full).unwrap(),
            "ram 25% 1.0G/4.0G  swap 1% 1.0M/100M  cache 1.0G  buffers 28M"
        );
        let sparse = section(vec![
            Item::new("ram", Value::Missing),
            Item::new("swap", Value::Usage { used: 0, total: 0 }),
            Item::new("cache", Value::Missing),
            Item::new("buffers", Value::Missing),
        ]);
        assert_eq!(memory_line(&sparse).unwrap(), "ram ?  swap none");
        let failed = vec![Section::failed(SectionId::Memory, "unreadable")];
        assert_eq!(memory_line(&failed).unwrap(), "ram ?");
        assert_eq!(memory_line(&[]), None);
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
