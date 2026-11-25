//! Watched services: systemd units over D-Bus, and processes by name.

use crate::collect::{CollectError, Collector, Ctx};
use crate::config::{self, Watched};
use crate::model::{Item, SectionId, ServiceState, Value};
use crate::parse::comm;
use crate::systemd::UnitStatus;

/// The `services` section: one row per watched entry, in the config's order.
/// A unit is labelled with its name, `.service` left out (`sshd`); a process
/// with its name. Nothing watched: no row, and systemd is not asked.
#[derive(Debug)]
pub struct Services {
    watch: Vec<Watched>,
}

impl Services {
    /// Watches the entries of `[services]`.
    #[must_use]
    pub fn new(config: &config::Services) -> Self {
        Self {
            watch: config.watch.clone(),
        }
    }
}

impl Collector for Services {
    fn id(&self) -> SectionId {
        SectionId::Services
    }

    /// Never fails as a whole. Without systemd (containers, other init
    /// systems) or when the call fails, the unit rows are `?`, while
    /// `process:` rows are still answered from `/proc`.
    fn collect(&self, ctx: &Ctx) -> Result<Vec<Item>, CollectError> {
        let units: Vec<String> = self
            .watch
            .iter()
            .filter_map(|w| match w {
                Watched::Unit(name) => Some(name.clone()),
                Watched::Process(_) => None,
            })
            .collect();
        // One round trip for every unit.
        let answer = if units.is_empty() {
            None
        } else {
            ctx.systemd.query(&units, false).ok()
        };
        let wanted: Vec<&str> = self
            .watch
            .iter()
            .filter_map(|w| match w {
                Watched::Process(name) => Some(name.as_str()),
                Watched::Unit(_) => None,
            })
            .collect();
        let found = if wanted.is_empty() {
            None
        } else {
            running_processes(ctx, &wanted)
        };

        Ok(self
            .watch
            .iter()
            .map(|watched| match watched {
                Watched::Unit(name) => {
                    let value = answer
                        .as_ref()
                        .and_then(|a| a.watched.iter().find(|u| u.name == *name))
                        .map_or(Value::Missing, |unit| Value::State(unit_state(unit)));
                    Item::new(name.strip_suffix(".service").unwrap_or(name), value)
                }
                Watched::Process(name) => {
                    let value = found.as_ref().map_or(Value::Missing, |found| {
                        Value::State(if found.contains(&name.as_str()) {
                            ServiceState::Running
                        } else {
                            ServiceState::Inactive
                        })
                    });
                    Item::new(name.as_str(), value)
                }
            })
            .collect())
    }
}

/// What a unit's states mean for someone who asked to watch it.
///
/// - `active` (or `reloading`) is `running`, or `exited` for a finished
///   oneshot. Units without a process (sockets `listening`, timers
///   `waiting`, mounts `mounted`) are `running` too: they do their job.
/// - `activating` and `deactivating` are `inactive`: a unit that crashes
///   and restarts in a loop stays in `activating` (`auto-restart`), and it
///   must not look healthy. A slow start looks the same for a moment.
/// - not loaded because no unit file exists is `not-found`; a masked or
///   broken unit is whatever its active state says (usually `inactive`).
fn unit_state(unit: &UnitStatus) -> ServiceState {
    if unit.load_state == "not-found" {
        return ServiceState::NotFound;
    }
    match unit.active_state.as_str() {
        "failed" => ServiceState::Failed,
        "active" | "reloading" | "refreshing" if unit.sub_state == "exited" => ServiceState::Exited,
        "active" | "reloading" | "refreshing" => ServiceState::Running,
        _ => ServiceState::Inactive,
    }
}

/// The `wanted` names that some process carries, from `/proc/<pid>/comm`.
/// `None` when `/proc` cannot be listed. The scan stops once every name is
/// found: opening one file per process costs about 5–15 µs, so a full scan
/// takes milliseconds on a busy machine. A process that exits during the
/// scan is skipped.
///
/// Kernel threads are not told apart (that needs one more file per
/// process); a watched name is a program's, so a clash is unlikely.
fn running_processes<'a>(ctx: &Ctx, wanted: &[&'a str]) -> Option<Vec<&'a str>> {
    let names = ctx.sysroot.read_dir_names("/proc").ok()?;
    let mut found: Vec<&'a str> = Vec::with_capacity(wanted.len());
    for pid in names {
        if found.len() == wanted.len() {
            break;
        }
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // 15 bytes and a line break: 64 is plenty.
        let Ok(text) = ctx.sysroot.read_small::<64>(&format!("/proc/{pid}/comm")) else {
            continue;
        };
        let Ok(name) = comm::parse(&text) else {
            continue;
        };
        if let Some(&hit) = wanted.iter().find(|&&w| w == name)
            && !found.contains(&hit)
        {
            found.push(hit);
        }
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use crate::sysroot::Sysroot;
    use crate::systemd::{FakeSystemd, NoSystemd, Systemd, SystemdError, UnitsAnswer};
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn ctx_with(root: impl Into<PathBuf>, systemd: Box<dyn Systemd>) -> Ctx {
        let sysroot = Sysroot::new(root);
        let (sys, _) = Fake::load(sysroot.root())
            .unwrap_or_default()
            .into_backends();
        Ctx {
            sysroot,
            sys,
            systemd,
        }
    }

    fn services(entries: &[&str]) -> Services {
        Services {
            watch: entries
                .iter()
                .map(|e| Watched::try_from((*e).to_owned()).unwrap())
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

    fn unit(name: &str, load: &str, active: &str, sub: &str) -> UnitStatus {
        UnitStatus {
            name: name.into(),
            description: String::new(),
            load_state: load.into(),
            active_state: active.into(),
            sub_state: sub.into(),
        }
    }

    /// Counts the queries, to check that nothing is asked when no unit is
    /// watched.
    #[derive(Debug, Default, Clone)]
    struct Counting(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl Systemd for Counting {
        fn query(&self, _: &[String], _: bool) -> Result<UnitsAnswer, SystemdError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(SystemdError::Call("counted".into()))
        }
    }

    #[test]
    fn unit_states() {
        let cases = [
            (("loaded", "active", "running"), ServiceState::Running),
            (("loaded", "active", "exited"), ServiceState::Exited),
            (("loaded", "active", "listening"), ServiceState::Running),
            (("loaded", "active", "waiting"), ServiceState::Running),
            (("loaded", "reloading", "reload"), ServiceState::Running),
            (("loaded", "failed", "failed"), ServiceState::Failed),
            (("loaded", "inactive", "dead"), ServiceState::Inactive),
            (
                ("loaded", "activating", "auto-restart"),
                ServiceState::Inactive,
            ),
            (("loaded", "activating", "start"), ServiceState::Inactive),
            (("loaded", "deactivating", "stop"), ServiceState::Inactive),
            (
                ("loaded", "maintenance", "cleaning"),
                ServiceState::Inactive,
            ),
            (("masked", "inactive", "dead"), ServiceState::Inactive),
            (("not-found", "inactive", "dead"), ServiceState::NotFound),
            // A unit file removed while the unit had failed.
            (("not-found", "failed", "failed"), ServiceState::NotFound),
            (("loaded", "something-new", "x"), ServiceState::Inactive),
        ];
        for ((load, active, sub), expected) in cases {
            assert_eq!(
                unit_state(&unit("a.service", load, active, sub)),
                expected,
                "{load} {active} {sub}"
            );
        }
    }

    #[test]
    fn units_and_processes_in_config_order() {
        let systemd = FakeSystemd {
            units: vec![
                unit("ssh.service", "loaded", "active", "running"),
                unit("backup.service", "loaded", "failed", "failed"),
                unit("ssh.socket", "loaded", "active", "listening"),
            ],
        };
        let items = services(&[
            "process:sshd",
            "ssh",
            "backup",
            "ssh.socket",
            "nope",
            "process:nginx",
            "process:chronyd",
        ])
        .collect(&ctx_with(fixture("rpi4-ubuntu"), Box::new(systemd)))
        .unwrap();
        assert_eq!(
            rows(&items),
            [
                ("sshd", "running"),
                ("ssh", "running"),
                ("backup", "failed"),
                ("ssh.socket", "running"),
                ("nope", "not-found"),
                ("nginx", "inactive"),
                ("chronyd", "running"),
            ]
        );
    }

    #[test]
    fn without_systemd_only_processes_answer() {
        // The container fixture has no systemd, and one process: sleep.
        let items = services(&["sshd", "process:sleep", "process:sshd"])
            .collect(&ctx_with(fixture("container"), Box::new(NoSystemd)))
            .unwrap();
        assert_eq!(
            rows(&items),
            [("sshd", "?"), ("sleep", "running"), ("sshd", "inactive")]
        );
    }

    #[test]
    fn failed_call_gives_unknown_units() {
        let items = services(&["sshd", "cron.timer"])
            .collect(&ctx_with("/nonexistent", Box::<Counting>::default()))
            .unwrap();
        assert_eq!(rows(&items), [("sshd", "?"), ("cron.timer", "?")]);
    }

    #[test]
    fn unlistable_proc_gives_unknown_processes() {
        let items = services(&["process:sshd"])
            .collect(&ctx_with("/nonexistent", Box::new(NoSystemd)))
            .unwrap();
        assert_eq!(rows(&items), [("sshd", "?")]);
    }

    #[test]
    fn nothing_watched_asks_nothing() {
        let counting = Counting::default();
        let ctx = ctx_with("/nonexistent", Box::new(counting.clone()));
        assert_eq!(services(&[]).collect(&ctx).unwrap(), []);
        assert_eq!(services(&["process:sshd"]).collect(&ctx).unwrap().len(), 1);
        assert_eq!(counting.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        services(&["sshd", "cron"]).collect(&ctx).unwrap();
        assert_eq!(
            counting.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "one query for every unit"
        );
    }

    #[test]
    fn scan_skips_odd_entries_and_stops_early() {
        let dir =
            std::env::temp_dir().join(format!("motd-rs-services-{}-scan", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files = [
            ("proc/1/comm", "init\n"),
            ("proc/2/comm", "\n"),
            ("proc/3/comm", "a-much-longer-name\n"),
            ("proc/self/comm", "sshd\n"),
            ("proc/4/comm", "Web Content\n"),
        ];
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        std::fs::create_dir_all(dir.join("proc/5")).unwrap(); // exited: no comm
        let ctx = ctx_with(&dir, Box::new(NoSystemd));
        let mut found = running_processes(&ctx, &["Web Content", "init", "sshd"]).unwrap();
        found.sort_unstable();
        assert_eq!(found, ["Web Content", "init"], "`self` is not a pid");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
