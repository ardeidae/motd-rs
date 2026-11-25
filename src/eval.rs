//! Thresholds → [`Status`]. The only place where a value is judged: collectors
//! report raw facts and renderers only show the status decided here.

use crate::config::{Levels, Thresholds};
use crate::model::{Section, Status, Value};

/// Which load average decides the status: the 5-minute one. The 1-minute
/// average reacts to short bursts (the login itself makes one), and the
/// 15-minute one lags too much to describe the machine now.
const LOAD_AVERAGE: usize = 1;

/// Sets the status of every evaluated item, then each section's status.
pub fn evaluate(sections: &mut [Section], thresholds: &Thresholds) {
    for section in sections {
        for item in &mut section.items {
            if let Some(status) = judge(&item.value, thresholds) {
                item.status = status;
            }
        }
        section.refresh_status();
    }
}

/// `None` for values that have no threshold (they keep their status).
fn judge(value: &Value, thresholds: &Thresholds) -> Option<Status> {
    match *value {
        Value::Load { hundredths, cpus } => Some(load(hundredths, cpus, &thresholds.load)),
        _ => None,
    }
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
