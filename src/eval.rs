//! Thresholds → [`Status`]. The only place where a value is judged: collectors
//! report raw facts and renderers only show the status decided here.

use crate::config::{Levels, Thresholds};
use crate::model::{Section, SectionId, Status, Value, memory_items};

/// Which load average decides the status: the 5-minute one. The 1-minute
/// average reacts to short bursts (the login itself makes one), and the
/// 15-minute one lags too much to describe the machine now.
const LOAD_AVERAGE: usize = 1;

/// Sets the status of every evaluated item, then each section's status.
pub fn evaluate(sections: &mut [Section], thresholds: &Thresholds) {
    for section in sections {
        for item in &mut section.items {
            if let Some(status) = judge(section.id, &item.label, &item.value, thresholds) {
                item.status = status;
            }
        }
        section.refresh_status();
    }
}

/// `None` for values that have no threshold (they keep their status). A
/// `Usage` is judged by what it measures: the same value type is used for
/// RAM, swap and, later, disks.
fn judge(
    section: SectionId,
    label: &str,
    value: &Value,
    thresholds: &Thresholds,
) -> Option<Status> {
    match (section, label, value) {
        (_, _, &Value::Load { hundredths, cpus }) => Some(load(hundredths, cpus, &thresholds.load)),
        (SectionId::Memory, memory_items::RAM, &Value::Usage { used, total }) => {
            usage(used, total, thresholds.memory)
        }
        (SectionId::Memory, memory_items::SWAP, &Value::Usage { used, total }) => {
            usage(used, total, thresholds.swap)
        }
        _ => None,
    }
}

/// The used percentage, rounded down, against percent levels: a row shown
/// as `79%` is never in warning at 80. `None` when `total` is 0 (nothing to
/// fill, such as a machine without swap): no threshold applies.
fn usage(used: u64, total: u64, levels: Levels<u8>) -> Option<Status> {
    let percent = (u128::from(used) * 100).checked_div(u128::from(total))?;
    Some(level(
        percent,
        &Levels {
            warn: u128::from(levels.warn),
            crit: u128::from(levels.crit),
        },
    ))
}

/// The load average per online CPU against the levels. Unknown when the CPU
/// count is: guessing 1 would raise false alarms on big machines.
fn load(hundredths: [u32; 3], cpus: u32, levels: &Levels<f64>) -> Status {
    let Some(&average) = hundredths.get(LOAD_AVERAGE) else {
        return Status::Unknown;
    };
    if cpus == 0 {
        return Status::Unknown;
    }
    level(f64::from(average) / 100.0 / f64::from(cpus), levels)
}

/// At or above `crit` → Crit, at or above `warn` → Warn, else Ok. So a level
/// of 0 always gives at least a warning, as the config file says.
fn level<T: PartialOrd + Copy>(value: T, levels: &Levels<T>) -> Status {
    if value >= levels.crit {
        Status::Crit
    } else if value >= levels.warn {
        Status::Warn
    } else {
        Status::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Item, SectionId};

    fn levels(warn: f64, crit: f64) -> Levels<f64> {
        Levels { warn, crit }
    }

    #[test]
    fn load_is_per_cpu_on_the_5_minute_average() {
        let l = levels(1.0, 2.0);
        // rpi4-ubuntu: 2.67 over 4 CPUs = 0.67.
        assert_eq!(load([156, 267, 305], 4, &l), Status::Ok);
        // A burst in the last minute alone does not warn.
        assert_eq!(load([900, 100, 100], 2, &l), Status::Ok);
        assert_eq!(load([0, 400, 0], 4, &l), Status::Warn, "exactly warn");
        assert_eq!(load([0, 799, 0], 4, &l), Status::Warn);
        assert_eq!(load([0, 800, 0], 4, &l), Status::Crit, "exactly crit");
        assert_eq!(
            load([0, 100, 0], 0, &l),
            Status::Unknown,
            "unknown CPU count"
        );
    }

    #[test]
    fn zero_levels_always_warn() {
        assert_eq!(load([0, 0, 0], 4, &levels(0.0, 5.0)), Status::Warn);
        assert_eq!(load([0, 0, 0], 4, &levels(0.0, 0.0)), Status::Crit);
    }

    #[test]
    fn usage_is_rounded_down_percent() {
        let l = Levels { warn: 80, crit: 90 };
        assert_eq!(usage(0, 100, l), Some(Status::Ok));
        assert_eq!(usage(7999, 10_000, l), Some(Status::Ok), "79.99 %");
        assert_eq!(usage(80, 100, l), Some(Status::Warn), "exactly warn");
        assert_eq!(usage(90, 100, l), Some(Status::Crit), "exactly crit");
        assert_eq!(usage(150, 100, l), Some(Status::Crit), "over 100 %");
        assert_eq!(usage(u64::MAX, u64::MAX, l), Some(Status::Crit));
        assert_eq!(usage(0, 0, l), None, "nothing to fill");
        assert_eq!(usage(0, 1, Levels { warn: 0, crit: 0 }), Some(Status::Crit));
    }

    #[test]
    fn memory_rows_use_their_own_levels() {
        let thresholds = Thresholds::default(); // memory 80/90, swap 10/20
        let mut sections = [Section::new(
            SectionId::Memory,
            vec![
                Item::new(
                    "ram",
                    Value::Usage {
                        used: 50,
                        total: 100,
                    },
                ),
                Item::new(
                    "swap",
                    Value::Usage {
                        used: 15,
                        total: 100,
                    },
                ),
                Item::new("cache", Value::Bytes(u64::MAX)),
                Item::new("buffers", Value::Missing),
            ],
        )];
        evaluate(&mut sections, &thresholds);
        let [section] = &sections;
        let statuses: Vec<Status> = section.items.iter().map(|i| i.status).collect();
        assert_eq!(
            statuses,
            [Status::Ok, Status::Warn, Status::Ok, Status::Unknown]
        );
        assert_eq!(section.status, Status::Warn);
    }

    #[test]
    fn no_swap_is_ok() {
        let mut sections = [Section::new(
            SectionId::Memory,
            vec![Item::new("swap", Value::Usage { used: 0, total: 0 })],
        )];
        evaluate(&mut sections, &Thresholds::default());
        assert_eq!(sections[0].status, Status::Ok);
    }

    #[test]
    fn usage_outside_memory_is_not_judged_as_memory() {
        // The label alone does not pick the levels: a row called "ram" in
        // another section is not RAM.
        assert_eq!(
            judge(
                SectionId::Users,
                "ram",
                &Value::Usage {
                    used: 99,
                    total: 100
                },
                &Thresholds::default()
            ),
            None
        );
    }

    #[test]
    fn evaluate_sets_items_and_section() {
        let mut sections = [Section::new(
            SectionId::Load,
            vec![
                Item::new(
                    "load",
                    Value::Load {
                        hundredths: [0, 900, 0],
                        cpus: 4,
                    },
                ),
                Item::new("procs", Value::Count(98)),
                Item::new("kernel_threads", Value::Missing),
            ],
        )];
        evaluate(&mut sections, &Thresholds::default());
        let [section] = &sections;
        assert_eq!(section.items[0].status, Status::Crit);
        assert_eq!(section.items[1].status, Status::Ok, "no threshold");
        assert_eq!(
            section.items[2].status,
            Status::Unknown,
            "missing stays unknown"
        );
        assert_eq!(section.status, Status::Crit);
    }
}
