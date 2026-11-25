//! `/proc/meminfo`: one `Key:   value kB` line per counter, in kibibytes
//! (the kernel's `kB` means 1024 bytes). Only the counters the memory
//! section needs are kept, converted to bytes.

use super::ParseError;

const SOURCE: &str = "/proc/meminfo";

/// The counters used by the memory section, in bytes. `None` = the line is
/// absent (older or stripped-down kernels, virtualised `/proc` in some
/// containers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Meminfo {
    /// `MemTotal`: usable RAM.
    pub total: u64,
    /// `MemAvailable`: the kernel's estimate of what can be allocated without
    /// swapping, page cache and reclaimable slab included.
    pub available: Option<u64>,
    /// `Buffers`: block device cache.
    pub buffers: Option<u64>,
    /// `Cached`: page cache (tmpfs and shared memory included).
    pub cached: Option<u64>,
    /// `SReclaimable`: slab memory the kernel can free (dentries, inodes).
    pub reclaimable: Option<u64>,
    /// `SwapTotal`. `Some(0)` = no swap configured.
    pub swap_total: Option<u64>,
    /// `SwapFree`.
    pub swap_free: Option<u64>,
}

/// Reads the counters of [`Meminfo`]. Unknown lines are ignored; the first
/// occurrence of a key wins.
///
/// # Errors
///
/// No `MemTotal` line, or a kept counter whose value is not `<digits> kB`
/// (or does not fit in a `u64` once in bytes).
pub fn parse(input: &str) -> Result<Meminfo, ParseError> {
    let mut total = None;
    let mut info = Meminfo::default();
    for line in input.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let slot = match key {
            "MemTotal" => &mut total,
            "MemAvailable" => &mut info.available,
            "Buffers" => &mut info.buffers,
            "Cached" => &mut info.cached,
            "SReclaimable" => &mut info.reclaimable,
            "SwapTotal" => &mut info.swap_total,
            "SwapFree" => &mut info.swap_free,
            _ => continue,
        };
        if slot.is_none() {
            *slot = Some(bytes(rest).ok_or_else(|| {
                ParseError::new(SOURCE, format!("'{key}:{rest}' is not a size in kB"))
            })?);
        }
    }
    info.total = total.ok_or_else(|| ParseError::new(SOURCE, "no MemTotal line"))?;
    Ok(info)
}

/// `"   7988412 kB"` → 7988412 × 1024.
fn bytes(field: &str) -> Option<u64> {
    let mut words = field.split_ascii_whitespace();
    let (Some(number), Some("kB"), None) = (words.next(), words.next(), words.next()) else {
        return None;
    };
    if !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    number.parse::<u64>().ok()?.checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIB: u64 = 1024;

    /// rpi4-ubuntu, cut: no swap.
    const RPI4: &str = "MemTotal:        7988412 kB
MemFree:          475948 kB
MemAvailable:    3624796 kB
Buffers:          249732 kB
Cached:          2528348 kB
SwapCached:            0 kB
Shmem:             39860 kB
SReclaimable:     735552 kB
SwapTotal:             0 kB
SwapFree:              0 kB
HugePages_Total:       0
Hugepagesize:       2048 kB
";

    #[test]
    fn real_input() {
        assert_eq!(
            parse(RPI4),
            Ok(Meminfo {
                total: 7_988_412 * KIB,
                available: Some(3_624_796 * KIB),
                buffers: Some(249_732 * KIB),
                cached: Some(2_528_348 * KIB),
                reclaimable: Some(735_552 * KIB),
                swap_total: Some(0),
                swap_free: Some(0),
            })
        );
    }

    #[test]
    fn only_mem_total_is_required() {
        assert_eq!(
            parse("MemTotal: 1024 kB"),
            Ok(Meminfo {
                total: 1024 * KIB,
                ..Meminfo::default()
            })
        );
    }

    #[test]
    fn first_occurrence_wins() {
        let info = parse("MemTotal: 2 kB\nMemTotal: 3 kB\n").unwrap();
        assert_eq!(info.total, 2 * KIB);
    }

    #[test]
    fn unknown_and_odd_lines_are_ignored() {
        // Values of unused keys are not checked.
        let info = parse("no colon here\nHugePages_Total: x\nMemTotal: 1 kB\n\n").unwrap();
        assert_eq!(info.total, KIB);
    }

    #[test]
    fn malformed_input() {
        for bad in [
            "",
            "\n",
            "MemFree: 10 kB\n",
            "MemTotal:\n",
            "MemTotal: 10\n",
            "MemTotal: 10 MB\n",
            "MemTotal: 10 kB extra\n",
            "MemTotal: -10 kB\n",
            "MemTotal: +10 kB\n",
            "MemTotal: 1.5 kB\n",
            "MemTotal: 18014398509481984 kB\n",
            "MemTotal: 99999999999999999999 kB\n",
            "MemTotal: 10 kB\nSwapFree: x kB\n",
            // Truncated in the middle of a line.
            "MemTotal: 10 kB\nMemAvailable: 5",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
