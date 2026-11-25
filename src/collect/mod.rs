//! Collectors: read the system (through [`crate::sysroot::Sysroot`],
//! [`crate::sys::Sys`] and [`crate::systemd::Systemd`]) and return raw facts.
//!
//! [`run`] starts one detached thread per collector and waits for their
//! answers until a global deadline. A collector that fails, crashes or is
//! still running at the deadline only turns its own section `Unknown`: a
//! login must never wait on a stuck `statvfs` or a slow D-Bus peer.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::model::{Item, Section, SectionId, Timing};
use crate::parse::ParseError;
use crate::sys::Sys;
use crate::sysroot::Sysroot;
use crate::systemd::Systemd;

pub mod disks;
pub mod host;
pub mod load;
pub mod memory;
pub mod network;
pub mod ports;
pub mod services;
pub mod users;

/// How long [`run`] waits for all collectors by default. Long enough for a
/// D-Bus round trip on a loaded Raspberry Pi, short enough to go unnoticed at
/// login.
pub const DEFAULT_DEADLINE: Duration = Duration::from_millis(250);

/// `Section::error` of a collector still running at the deadline.
pub const TIMEOUT: &str = "timeout";
/// `Section::error` of a collector whose thread ended without answering
/// (a panic in a debug build), or could not be started.
pub const CRASHED: &str = "crashed";

/// Everything a collector may read from. Shared by all collector threads.
#[derive(Debug)]
pub struct Ctx {
    /// Root of every file read.
    pub sysroot: Sysroot,
    /// Syscalls.
    pub sys: Box<dyn Sys>,
    /// systemd unit states.
    pub systemd: Box<dyn Systemd>,
}

/// Why a collector produced nothing.
#[derive(Debug)]
pub enum CollectError {
    /// A file it cannot work without is unreadable.
    Io {
        /// System path of the file (`/proc/meminfo`).
        path: &'static str,
        /// The underlying error.
        source: io::Error,
    },
    /// A file it cannot work without is malformed.
    Parse(ParseError),
    /// The data source does not exist on this system (no systemd, …).
    Unavailable(&'static str),
}

impl CollectError {
    /// Short reason stored in [`Section::error`] and shown next to `?`.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::Io { .. } => "unreadable",
            Self::Parse(_) => "unparsable",
            Self::Unavailable(what) => what,
        }
    }
}

impl fmt::Display for CollectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot read {path}: {source}"),
            Self::Parse(err) => err.fmt(f),
            Self::Unavailable(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for CollectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse(err) => Some(err),
            Self::Unavailable(_) => None,
        }
    }
}

impl From<ParseError> for CollectError {
    fn from(err: ParseError) -> Self {
        Self::Parse(err)
    }
}

/// One section's data source.
pub trait Collector: Send + Sync + 'static {
    /// The section it fills.
    fn id(&self) -> SectionId;

    /// Reads the raw facts. Items are not evaluated yet: thresholds are
    /// applied afterwards, so collectors never decide a status.
    ///
    /// # Errors
    ///
    /// The section cannot be shown at all. A single unreadable row is a
    /// [`crate::model::Value::Missing`] item instead.
    fn collect(&self, ctx: &Ctx) -> Result<Vec<Item>, CollectError>;
}

/// What [`run`] gathered.
#[derive(Debug, Default)]
pub struct Collected {
    /// One section per collector, in the order the collectors were given
    /// (not the order they finished: the output must not depend on timing).
    pub sections: Vec<Section>,
    /// One timing per collector, same order.
    pub timings: Vec<Timing>,
}

/// Runs every collector in its own thread and waits at most `deadline`.
///
/// Threads are detached: a collector stuck in the kernel is abandoned, its
/// section reports [`TIMEOUT`], and the process exits without joining it.
#[must_use]
pub fn run(collectors: Vec<Box<dyn Collector>>, ctx: &Arc<Ctx>, deadline: Duration) -> Collected {
    let start = Instant::now();
    let ids: Vec<SectionId> = collectors.iter().map(|c| c.id()).collect();
    let mut answers: Vec<Option<(Section, Duration)>> = ids.iter().map(|_| None).collect();
    let (tx, rx) = mpsc::channel();

    let mut pending = 0_usize;
    for (index, collector) in collectors.into_iter().enumerate() {
        let id = collector.id();
        let tx = tx.clone();
        let ctx = Arc::clone(ctx);
        let spawned = thread::Builder::new()
            .name(format!("collect-{id}"))
            .spawn(move || {
                let section = match collector.collect(&ctx) {
                    Ok(items) => Section::new(id, items),
                    Err(err) => Section::failed(id, err.reason()),
                };
                // The receiver is gone once the deadline has passed: nobody
                // wants this answer any more.
                let _ = tx.send((index, section, start.elapsed()));
            });
        match spawned {
            Ok(_) => pending += 1,
            Err(_) => {
                if let Some(slot) = answers.get_mut(index) {
                    *slot = Some((Section::failed(id, CRASHED), start.elapsed()));
                }
            }
        }
    }
    // Only the threads hold senders now, so a disconnect means none of the
    // remaining ones will ever answer.
    drop(tx);

    let mut late = TIMEOUT;
    while pending > 0 {
        let left = deadline.saturating_sub(start.elapsed());
        match rx.recv_timeout(left) {
            Ok((index, section, elapsed)) => {
                if let Some(slot) = answers.get_mut(index) {
                    *slot = Some((section, elapsed));
                }
                pending -= 1;
            }
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => {
                late = CRASHED;
                break;
            }
        }
    }

    let given_up = start.elapsed();
    let mut collected = Collected::default();
    for (id, answer) in ids.into_iter().zip(answers) {
        let (section, elapsed) = answer.unwrap_or_else(|| (Section::failed(id, late), given_up));
        collected.sections.push(section);
        collected.timings.push(Timing {
            section: id,
            elapsed,
        });
    }
    collected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Status, Value};
    use crate::sys::FakeSys;
    use crate::systemd::NoSystemd;
    use std::sync::Mutex;

    fn ctx() -> Arc<Ctx> {
        Arc::new(Ctx {
            sysroot: Sysroot::new("/nonexistent-motd-rs-root"),
            sys: Box::new(FakeSys::default()),
            systemd: Box::new(NoSystemd),
        })
    }

    /// Answers one `Text` item after an optional delay.
    struct Answers(SectionId, Duration);

    impl Collector for Answers {
        fn id(&self) -> SectionId {
            self.0
        }
        fn collect(&self, _: &Ctx) -> Result<Vec<Item>, CollectError> {
            thread::sleep(self.1);
            Ok(vec![Item::new("row", Value::Text(self.0.to_string()))])
        }
    }

    struct Fails(SectionId);

    impl Collector for Fails {
        fn id(&self) -> SectionId {
            self.0
        }
        fn collect(&self, _: &Ctx) -> Result<Vec<Item>, CollectError> {
            Err(CollectError::Io {
                path: "/proc/meminfo",
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            })
        }
    }

    /// Blocks until the test releases it, like a `statvfs` on a dead NFS server.
    struct Stuck(SectionId, Mutex<mpsc::Receiver<()>>);

    impl Collector for Stuck {
        fn id(&self) -> SectionId {
            self.0
        }
        fn collect(&self, _: &Ctx) -> Result<Vec<Item>, CollectError> {
            if let Ok(release) = self.1.lock() {
                let _ = release.recv();
            }
            Ok(Vec::new())
        }
    }

    struct Panics(SectionId);

    impl Collector for Panics {
        fn id(&self) -> SectionId {
            self.0
        }
        fn collect(&self, _: &Ctx) -> Result<Vec<Item>, CollectError> {
            panic!("collector bug");
        }
    }

    fn ids(collected: &Collected) -> Vec<SectionId> {
        collected.sections.iter().map(|s| s.id).collect()
    }

    #[test]
    fn keeps_the_given_order_whatever_finishes_first() {
        let collectors: Vec<Box<dyn Collector>> = vec![
            Box::new(Answers(SectionId::Disks, Duration::from_millis(30))),
            Box::new(Answers(SectionId::Load, Duration::ZERO)),
        ];
        let collected = run(collectors, &ctx(), Duration::from_secs(10));
        assert_eq!(ids(&collected), [SectionId::Disks, SectionId::Load]);
        assert!(collected.sections.iter().all(|s| s.error.is_none()));
        assert_eq!(
            collected.sections[0].items[0].value,
            Value::Text("disks".into())
        );
        let timed: Vec<_> = collected.timings.iter().map(|t| t.section).collect();
        assert_eq!(timed, ids(&collected));
        assert!(collected.timings[0].elapsed >= Duration::from_millis(30));
    }

    #[test]
    fn a_failure_only_affects_its_own_section() {
        let collectors: Vec<Box<dyn Collector>> = vec![
            Box::new(Fails(SectionId::Memory)),
            Box::new(Answers(SectionId::Load, Duration::ZERO)),
        ];
        let collected = run(collectors, &ctx(), Duration::from_secs(10));
        let memory = &collected.sections[0];
        assert_eq!(memory.status, Status::Unknown);
        assert_eq!(memory.error, Some("unreadable"));
        assert_eq!(memory.items.len(), 0);
        assert_eq!(collected.sections[1].error, None);
        assert_eq!(collected.sections[1].items.len(), 1);
    }

    #[test]
    fn a_stuck_collector_times_out_without_delaying_the_others() {
        let (release, wait) = mpsc::channel();
        let collectors: Vec<Box<dyn Collector>> = vec![
            Box::new(Stuck(SectionId::Disks, Mutex::new(wait))),
            Box::new(Answers(SectionId::Load, Duration::ZERO)),
        ];
        let deadline = Duration::from_millis(50);
        let start = Instant::now();
        let collected = run(collectors, &ctx(), deadline);
        let took = start.elapsed();
        let _ = release.send(());

        assert!(took >= deadline);
        assert!(took < Duration::from_secs(5), "runner waited {took:?}");
        let disks = &collected.sections[0];
        assert_eq!(disks.error, Some(TIMEOUT));
        assert_eq!(disks.status, Status::Unknown);
        assert!(collected.timings[0].elapsed >= deadline);
        assert_eq!(collected.sections[1].error, None);
    }

    #[test]
    fn a_crashed_collector_is_reported_not_waited_for() {
        let collectors: Vec<Box<dyn Collector>> = vec![
            Box::new(Panics(SectionId::Ports)),
            Box::new(Answers(SectionId::Load, Duration::ZERO)),
        ];
        let start = Instant::now();
        let collected = run(collectors, &ctx(), Duration::from_secs(10));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(collected.sections[0].error, Some(CRASHED));
        assert_eq!(collected.sections[1].error, None);
    }

    #[test]
    fn no_collectors_returns_at_once() {
        let start = Instant::now();
        let collected = run(Vec::new(), &ctx(), Duration::from_secs(10));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(collected.sections.len(), 0);
        assert_eq!(collected.timings.len(), 0);
    }

    #[test]
    fn error_reasons() {
        let parse = CollectError::from(ParseError::new("/proc/loadavg", "empty"));
        assert_eq!(parse.reason(), "unparsable");
        assert_eq!(parse.to_string(), "cannot parse /proc/loadavg: empty");
        assert_eq!(
            CollectError::Unavailable("no systemd").reason(),
            "no systemd"
        );
        let io = CollectError::Io {
            path: "/proc/meminfo",
            source: io::Error::from(io::ErrorKind::NotFound),
        };
        assert!(io.to_string().starts_with("cannot read /proc/meminfo: "));
    }
}
