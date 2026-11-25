//! Load averages, online CPUs and process counts.

use crate::collect::{CollectError, Collector, Ctx};
use crate::model::{Item, SectionId, Value, load_items};
use crate::parse::{cpu_list, loadavg, pid_list, pid_stat};

/// The `load` section: one item per [`load_items`] key, in that order.
#[derive(Debug)]
pub struct Load;

impl Collector for Load {
    fn id(&self) -> SectionId {
        SectionId::Load
    }

    /// Each part is read on its own: an unreadable `/proc/loadavg` leaves the
    /// process counts, and the other way round.
    fn collect(&self, ctx: &Ctx) -> Result<Vec<Item>, CollectError> {
        let sysroot = &ctx.sysroot;
        let averages = sysroot
            .read_to_string("/proc/loadavg")
            .ok()
            .and_then(|text| loadavg::parse(&text).ok());
        // 0 = unknown: eval then reports the load as Unknown rather than
        // guessing a CPU count.
        let cpus = sysroot
            .read_to_string("/sys/devices/system/cpu/online")
            .ok()
            .and_then(|text| cpu_list::count(&text).ok())
            .unwrap_or(0);
        let load = averages.map_or(Value::Missing, |hundredths| Value::Load {
            hundredths,
            cpus,
        });

        let counts = count_processes(ctx);
        let count = |pick: fn(&Counts) -> u64| {
            counts
                .as_ref()
                .map_or(Value::Missing, |c| Value::Count(pick(c)))
        };
        Ok(vec![
            Item::new(load_items::LOAD, load),
            Item::new(load_items::PROCS, count(|c| c.processes)),
            Item::new(load_items::KERNEL_THREADS, count(|c| c.kernel_threads)),
        ])
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    /// Processes, kernel threads excluded.
    processes: u64,
    kernel_threads: u64,
}

/// `None` when `/proc` cannot be listed.
///
/// Opening one file per process costs about 5–15 µs in the kernel, so
/// reading every `/proc/<pid>/stat` takes several milliseconds on a machine
/// with a few hundred processes: most of the run-time budget. The total comes
/// from the listing alone, and the kernel threads from one file (see
/// [`kernel_threads_fast`]). The per-process scan is only the fallback.
fn count_processes(ctx: &Ctx) -> Option<Counts> {
    let pids: Vec<String> = ctx
        .sysroot
        .read_dir_names("/proc")
        .ok()?
        .into_iter()
        .filter(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
        .collect();
    let total = u64::try_from(pids.len()).unwrap_or(u64::MAX);
    let kernel_threads =
        kernel_threads_fast(ctx).unwrap_or_else(|| kernel_threads_scan(ctx, &pids));
    Some(Counts {
        // Saturating: the listing and the children list are two snapshots.
        processes: total.saturating_sub(kernel_threads),
        kernel_threads,
    })
}

/// Every kernel thread is created by `kthreadd` (pid 2), so they are
/// `kthreadd` plus its children, which the kernel lists in one file. `None`
/// when that file is missing (kernel without `CONFIG_PROC_CHILDREN`) or when
/// pid 2 is not a kernel thread (in a container, pid 2 is one of its own
/// processes).
fn kernel_threads_fast(ctx: &Ctx) -> Option<u64> {
    let stat = ctx.sysroot.read_small::<512>("/proc/2/stat").ok()?;
    if !pid_stat::parse(&stat).ok()?.kernel_thread {
        return None;
    }
    let children = ctx.sysroot.read_to_string("/proc/2/task/2/children").ok()?;
    pid_list::count(&children).ok()?.checked_add(1)
}

/// Reads `/proc/<pid>/stat` for every process and counts the kernel threads.
/// A process that exits between the listing and the read, or whose line
/// does not parse, is skipped: the count is a snapshot anyway.
fn kernel_threads_scan(ctx: &Ctx, pids: &[String]) -> u64 {
    let mut kernel_threads = 0;
    for pid in pids {
        // A stat line is about 300 bytes; the fields we read are in the first
        // ~60 after the name, and a name is at most 64 bytes.
        let Ok(text) = ctx.sysroot.read_small::<512>(&format!("/proc/{pid}/stat")) else {
            continue;
        };
        if pid_stat::parse(&text).is_ok_and(|stat| stat.kernel_thread) {
            kernel_threads += 1;
        }
    }
    kernel_threads
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use crate::sysroot::Sysroot;
    use std::path::{Path, PathBuf};

    fn collect_root(root: Sysroot) -> Vec<Item> {
        let (sys, systemd) = Fake::load(root.root()).unwrap_or_default().into_backends();
        let ctx = Ctx {
            sysroot: root,
            sys,
            systemd,
        };
        Load.collect(&ctx).unwrap()
    }

    fn fixture(name: &str) -> Vec<Item> {
        collect_root(Sysroot::new(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        )))
    }

    fn values(items: &[Item]) -> Vec<(&str, &Value)> {
        items.iter().map(|i| (i.label.as_str(), &i.value)).collect()
    }

    /// A temporary root with the given files.
    fn root(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("motd-rs-load-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    /// `(procs, kernel_threads)` of a temporary root, which is then removed.
    fn counts(dir: &Path) -> (Value, Value) {
        let items = collect_root(Sysroot::new(dir));
        std::fs::remove_dir_all(dir).unwrap();
        (items[1].value.clone(), items[2].value.clone())
    }

    const KTHREADD: &str = "2 (kthreadd) S 0 0 0 0 -1 2129984 0";
    const INIT: &str = "1 (init) S 0 1 1 0 -1 4194560 0";

    #[test]
    fn rpi4_ubuntu() {
        let items = fixture("rpi4-ubuntu");
        assert_eq!(
            values(&items),
            [
                (
                    "load",
                    &Value::Load {
                        hundredths: [156, 267, 305],
                        cpus: 4
                    }
                ),
                ("procs", &Value::Count(98)),
                ("kernel_threads", &Value::Count(144)),
            ]
        );
    }

    #[test]
    fn fast_path_and_scan_agree_on_a_real_machine() {
        // rpi4-ubuntu's children list was rebuilt from the ppid field of its
        // captured stat files (it was captured before the script took the
        // list): exactly what the kernel would have printed then.
        let sysroot = Sysroot::new(format!(
            "{}/tests/fixtures/rpi4-ubuntu",
            env!("CARGO_MANIFEST_DIR")
        ));
        let (sys, systemd) = Fake::load(sysroot.root()).unwrap().into_backends();
        let ctx = Ctx {
            sysroot,
            sys,
            systemd,
        };
        let pids: Vec<String> = ctx
            .sysroot
            .read_dir_names("/proc")
            .unwrap()
            .into_iter()
            .filter(|name| name.bytes().all(|b| b.is_ascii_digit()))
            .collect();
        assert_eq!(kernel_threads_fast(&ctx), Some(144));
        assert_eq!(kernel_threads_scan(&ctx, &pids), 144);
    }

    #[test]
    fn fast_path_reads_no_process_file() {
        // pid dirs 3..=6 have no stat file: a scan would not see them as
        // kernel threads, so 2 processes means the children list was used.
        let mut files = vec![
            ("proc/1/stat", INIT),
            ("proc/2/stat", KTHREADD),
            ("proc/2/task/2/children", "3 4 5 6 \n"),
            ("proc/7/stat", "7 (sshd) S 1 7 7 0 -1 4194560 0"),
        ];
        files.extend(["proc/3/x", "proc/4/x", "proc/5/x", "proc/6/x"].map(|d| (d, "")));
        let dir = root("fast", &files);
        assert_eq!(counts(&dir), (Value::Count(2), Value::Count(5)));
    }

    #[test]
    fn falls_back_to_the_scan() {
        let threads = [
            ("proc/1/stat", INIT),
            ("proc/2/stat", KTHREADD),
            ("proc/3/stat", "3 (kworker/0:0) I 2 0 0 0 -1 69238880 0"),
            ("proc/9/stat", "garbage"),
        ];
        // No children list (kernel without CONFIG_PROC_CHILDREN).
        let dir = root("no-children", &threads);
        assert_eq!(counts(&dir), (Value::Count(2), Value::Count(2)));
        // A children list that does not parse.
        let mut files = threads.to_vec();
        files.push(("proc/2/task/2/children", "3 x"));
        let dir = root("bad-children", &files);
        assert_eq!(counts(&dir), (Value::Count(2), Value::Count(2)));
    }

    #[test]
    fn container_pid_2_is_not_kthreadd() {
        // In a container pid 2 is an ordinary process: its children are not
        // kernel threads, whatever the list says.
        let dir = root(
            "container",
            &[
                ("proc/1/stat", INIT),
                ("proc/2/stat", "2 (bash) S 1 2 2 0 -1 4194304 0"),
                ("proc/2/task/2/children", "3 \n"),
                ("proc/3/stat", "3 (sleep) S 2 3 3 0 -1 4194304 0"),
            ],
        );
        assert_eq!(counts(&dir), (Value::Count(3), Value::Count(0)));
    }

    #[test]
    fn container_fixture() {
        let items = fixture("container");
        assert_eq!(items[1].value, Value::Count(1));
        assert_eq!(items[2].value, Value::Count(0));
        assert!(matches!(items[0].value, Value::Load { cpus: 20, .. }));
    }

    #[test]
    fn missing_files_give_missing_items() {
        let items = collect_root(Sysroot::new("/nonexistent-motd-rs-root"));
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| i.value == Value::Missing), "{items:?}");
    }

    #[test]
    fn unknown_cpu_count_is_zero() {
        let dir = root("cpus", &[("proc/loadavg", "0.50 0.25 0.10 1/3 10\n")]);
        let items = collect_root(Sysroot::new(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            items[0].value,
            Value::Load {
                hundredths: [50, 25, 10],
                cpus: 0
            }
        );
    }
}
