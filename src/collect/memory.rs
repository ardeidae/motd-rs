//! RAM, swap, cache and buffers from `/proc/meminfo`.

use crate::collect::{CollectError, Collector, Ctx};
use crate::model::{Item, SectionId, Value, memory_items};
use crate::parse::meminfo::{self, Meminfo};

/// The `memory` section: one item per [`memory_items`] key, in that order.
#[derive(Debug)]
pub struct Memory;

impl Collector for Memory {
    fn id(&self) -> SectionId {
        SectionId::Memory
    }

    /// Every row comes from the same file, so an unreadable or malformed
    /// `/proc/meminfo` fails the whole section.
    fn collect(&self, ctx: &Ctx) -> Result<Vec<Item>, CollectError> {
        let path = "/proc/meminfo";
        let text = ctx
            .sysroot
            .read_to_string(path)
            .map_err(|source| CollectError::Io { path, source })?;
        Ok(items(&meminfo::parse(&text)?))
    }
}

fn items(info: &Meminfo) -> Vec<Item> {
    let bytes = |value: Option<u64>| value.map_or(Value::Missing, Value::Bytes);
    vec![
        Item::new(memory_items::RAM, ram(info)),
        Item::new(memory_items::SWAP, swap(info)),
        // What free(1) calls "cache": slab the kernel can reclaim is as much
        // a cache as the page cache.
        Item::new(
            memory_items::CACHE,
            bytes(
                info.cached
                    .map(|cached| cached.saturating_add(info.reclaimable.unwrap_or(0))),
            ),
        ),
        Item::new(memory_items::BUFFERS, bytes(info.buffers)),
    ]
}

/// Used = `MemTotal - MemAvailable`. `MemFree` would count the page cache
/// as used and show a healthy machine as nearly full. Without `MemAvailable`
/// (kernels before 3.14) the row is unknown rather than wrong.
fn ram(info: &Meminfo) -> Value {
    match info.available {
        Some(available) if info.total > 0 => Value::Usage {
            // Saturating: the two counters are not read atomically.
            used: info.total.saturating_sub(available),
            total: info.total,
        },
        _ => Value::Missing,
    }
}

/// `total` 0 is a machine without swap (many VMs, containers, Raspberry Pi
/// images): a normal state, not an error.
fn swap(info: &Meminfo) -> Value {
    match (info.swap_total, info.swap_free) {
        (Some(0), _) => Value::Usage { used: 0, total: 0 },
        (Some(total), Some(free)) => Value::Usage {
            used: total.saturating_sub(free),
            total,
        },
        _ => Value::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use crate::sysroot::Sysroot;

    const KIB: u64 = 1024;

    fn collect_root(root: Sysroot) -> Result<Vec<Item>, CollectError> {
        let (sys, systemd) = Fake::load(root.root()).unwrap_or_default().into_backends();
        let ctx = Ctx {
            sysroot: root,
            sys,
            systemd,
        };
        Memory.collect(&ctx)
    }

    fn fixture(name: &str) -> Vec<(String, Value)> {
        collect_root(Sysroot::new(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        )))
        .unwrap()
        .into_iter()
        .map(|i| (i.label, i.value))
        .collect()
    }

    fn pairs(items: &[(&str, Value)]) -> Vec<(String, Value)> {
        items
            .iter()
            .map(|(l, v)| ((*l).to_owned(), v.clone()))
            .collect()
    }

    #[test]
    fn rpi4_ubuntu_has_no_swap() {
        assert_eq!(
            fixture("rpi4-ubuntu"),
            pairs(&[
                (
                    "ram",
                    Value::Usage {
                        used: (7_988_412 - 3_624_796) * KIB,
                        total: 7_988_412 * KIB,
                    }
                ),
                ("swap", Value::Usage { used: 0, total: 0 }),
                ("cache", Value::Bytes((2_528_348 + 735_552) * KIB)),
                ("buffers", Value::Bytes(249_732 * KIB)),
            ])
        );
    }

    #[test]
    fn container_has_swap() {
        let items = fixture("container");
        assert_eq!(
            items.get(1),
            Some(&(
                "swap".to_owned(),
                Value::Usage {
                    used: (41_943_036 - 34_722_300) * KIB,
                    total: 41_943_036 * KIB,
                }
            ))
        );
    }

    #[test]
    fn ram_ignores_mem_free() {
        // Counting only MemFree as free would show the page cache as used:
        // 94 % here instead of 50 %.
        let info = meminfo::parse(
            "MemTotal: 1000 kB\nMemFree: 60 kB\nMemAvailable: 500 kB\nCached: 440 kB\n",
        )
        .unwrap();
        assert_eq!(
            ram(&info),
            Value::Usage {
                used: 500 * KIB,
                total: 1000 * KIB
            }
        );
    }

    #[test]
    fn missing_counters() {
        let info = meminfo::parse("MemTotal: 1000 kB\n").unwrap();
        assert_eq!(
            items(&info)
                .into_iter()
                .map(|i| i.value)
                .collect::<Vec<_>>(),
            vec![Value::Missing; 4]
        );
        // Cache without SReclaimable: the page cache alone.
        let info = meminfo::parse("MemTotal: 1 kB\nCached: 3 kB\nSwapTotal: 8 kB\n").unwrap();
        let values: Vec<Value> = items(&info).into_iter().map(|i| i.value).collect();
        assert_eq!(values[1], Value::Missing, "swap total without free");
        assert_eq!(values[2], Value::Bytes(3 * KIB));
    }

    #[test]
    fn counters_out_of_step_do_not_underflow() {
        let info = meminfo::parse(
            "MemTotal: 10 kB\nMemAvailable: 11 kB\nSwapTotal: 4 kB\nSwapFree: 5 kB\n",
        )
        .unwrap();
        assert_eq!(
            ram(&info),
            Value::Usage {
                used: 0,
                total: 10 * KIB
            }
        );
        assert_eq!(
            swap(&info),
            Value::Usage {
                used: 0,
                total: 4 * KIB
            }
        );
    }

    #[test]
    fn zero_mem_total_is_unknown() {
        let info = meminfo::parse("MemTotal: 0 kB\nMemAvailable: 0 kB\n").unwrap();
        assert_eq!(ram(&info), Value::Missing);
    }

    #[test]
    fn unreadable_or_malformed_file_fails_the_section() {
        let err = collect_root(Sysroot::new("/nonexistent-motd-rs-root")).unwrap_err();
        assert_eq!(err.reason(), "unreadable");

        let dir = std::env::temp_dir().join(format!("motd-rs-memory-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("proc")).unwrap();
        std::fs::write(dir.join("proc/meminfo"), "MemFree: 1 kB\n").unwrap();
        let err = collect_root(Sysroot::new(&dir)).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(err.reason(), "unparsable");
    }
}
