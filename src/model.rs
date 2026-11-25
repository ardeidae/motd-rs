//! The report model: what collectors and evaluation produce and what renderers
//! consume. Pure data, no I/O and no formatting.
//!
//! Quantities stay raw (bytes, seconds): units and rounding are chosen by the
//! renderer, so text and JSON output never disagree.

use std::fmt;
use std::time::Duration;

/// Health of an item, a section or the whole report.
///
/// The order is severity: `Ok < Unknown < Warn < Crit`. An unreadable value
/// ranks above `Ok` so it is not hidden behind healthy ones, but below real
/// problems so it never masks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Status {
    /// Within thresholds, or informational.
    #[default]
    Ok,
    /// Could not be read or evaluated.
    Unknown,
    /// Past the warning threshold.
    Warn,
    /// Past the critical threshold, or down.
    Crit,
}

impl Status {
    /// The most severe status of `statuses`, `Ok` when there is none.
    #[must_use]
    pub fn worst(statuses: impl IntoIterator<Item = Self>) -> Self {
        statuses.into_iter().max().unwrap_or_default()
    }

    /// Stable lowercase name, used in JSON output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Unknown => "unknown",
            Self::Warn => "warn",
            Self::Crit => "crit",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Item labels of the [`SectionId::Load`] section. They are keys, not text:
/// renderers decide how each one is shown.
pub mod load_items {
    /// [`super::Value::Load`].
    pub const LOAD: &str = "load";
    /// Processes, kernel threads excluded ([`super::Value::Count`]).
    pub const PROCS: &str = "procs";
    /// Kernel threads ([`super::Value::Count`]). Not shown in the text
    /// output: their number depends on the hardware, not on what runs.
    pub const KERNEL_THREADS: &str = "kernel_threads";
}

/// Item labels of the [`SectionId::Memory`] section, in display order.
pub mod memory_items {
    /// RAM in use = `MemTotal - MemAvailable` ([`super::Value::Usage`]).
    pub const RAM: &str = "ram";
    /// Swap in use ([`super::Value::Usage`]). `total` 0 = no swap.
    pub const SWAP: &str = "swap";
    /// Page cache plus reclaimable slab ([`super::Value::Bytes`]),
    /// informational: memory the kernel gives back on demand.
    pub const CACHE: &str = "cache";
    /// Block device buffers ([`super::Value::Bytes`]), informational.
    pub const BUFFERS: &str = "buffers";
}

/// One section of the report. Each collector fills exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SectionId {
    /// Load average, online CPUs, process count.
    Load,
    /// RAM and swap.
    Memory,
    /// Mounted filesystems.
    Disks,
    /// Default route and addresses.
    Network,
    /// Logged-in users and sessions.
    Users,
    /// Listening ports.
    Ports,
    /// Watched systemd units and processes.
    Services,
}

impl SectionId {
    /// Every section, in a fixed order (tests, JSON, config validation).
    pub const ALL: [Self; 7] = [
        Self::Load,
        Self::Memory,
        Self::Disks,
        Self::Network,
        Self::Users,
        Self::Ports,
        Self::Services,
    ];

    /// Stable lowercase name: the key used in the config file and in JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Load => "load",
            Self::Memory => "memory",
            Self::Disks => "disks",
            Self::Network => "net",
            Self::Users => "users",
            Self::Ports => "ports",
            Self::Services => "services",
        }
    }

    /// The section with this config/JSON name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.as_str() == name)
    }
}

impl fmt::Display for SectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// State of a watched service, process or port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceState {
    /// Active with a running process (or a `process:` entry that was found).
    Running,
    /// Active without a process: a oneshot unit that has finished.
    Exited,
    /// In systemd's `failed` state.
    Failed,
    /// Loaded but not active (or a `process:` entry that was not found).
    Inactive,
    /// Unknown to systemd.
    NotFound,
    /// A watched port with a listening socket.
    Listening,
    /// A watched port with no listening socket.
    Closed,
}

impl ServiceState {
    /// Stable word shown in the output and in JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::Inactive => "inactive",
            Self::NotFound => "not-found",
            Self::Listening => "listening",
            Self::Closed => "closed",
        }
    }
}

impl fmt::Display for ServiceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The value of one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A filled capacity, in bytes (or inodes). Rendered as bar, percent, size.
    Usage {
        /// Amount in use.
        used: u64,
        /// Capacity. `0` means unknown or empty: no percentage can be shown.
        total: u64,
    },
    /// Load averages over 1, 5 and 15 minutes, in hundredths (`1.56` → 156:
    /// the kernel prints two decimals, so this is exact), and the number of
    /// online CPUs (`0` = unknown).
    Load {
        /// 1, 5 and 15 minutes.
        hundredths: [u32; 3],
        /// Online CPUs.
        cpus: u32,
    },
    /// A number of things (processes, …).
    Count(u64),
    /// An amount of bytes with no capacity to compare it with (cache, …).
    /// Informational: no threshold applies.
    Bytes(u64),
    /// State of a watched service or port.
    State(ServiceState),
    /// Free text taken from the system. Renderers sanitise it.
    Text(String),
    /// Expected but could not be read: rendered as `?`.
    Missing,
}

/// One row of a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Row label (mount point, unit name, `ram`, …), raw from the system.
    pub label: String,
    /// What is measured.
    pub value: Value,
    /// Set by evaluation; `Ok` until then.
    pub status: Status,
}

impl Item {
    /// An item not evaluated yet (`Ok`, or `Unknown` for [`Value::Missing`]).
    #[must_use]
    pub fn new(label: impl Into<String>, value: Value) -> Self {
        let status = if value == Value::Missing {
            Status::Unknown
        } else {
            Status::Ok
        };
        Self {
            label: label.into(),
            value,
            status,
        }
    }
}

/// One section: a titled column or header line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Which section.
    pub id: SectionId,
    /// Rows, in display order.
    pub items: Vec<Item>,
    /// Worst status of the items, or `Unknown` when the collector failed.
    pub status: Status,
    /// Why the collector produced nothing (`"timeout"`, …). A failed collector
    /// only affects its own section.
    pub error: Option<&'static str>,
}

impl Section {
    /// A section whose status is the worst of its items.
    #[must_use]
    pub fn new(id: SectionId, items: Vec<Item>) -> Self {
        let status = Status::worst(items.iter().map(|item| item.status));
        Self {
            id,
            items,
            status,
            error: None,
        }
    }

    /// A section whose collector failed or timed out.
    #[must_use]
    pub fn failed(id: SectionId, error: &'static str) -> Self {
        Self {
            id,
            items: Vec::new(),
            status: Status::Unknown,
            error: Some(error),
        }
    }

    /// Recomputes `status` after items were evaluated. A failed section stays
    /// `Unknown`.
    pub fn refresh_status(&mut self) {
        self.status = if self.error.is_some() {
            Status::Unknown
        } else {
            Status::worst(self.items.iter().map(|item| item.status))
        };
    }
}

/// Raw host facts. `None` = could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Host {
    /// Kernel hostname.
    pub hostname: Option<String>,
    /// `PRETTY_NAME` from os-release.
    pub os: Option<String>,
    /// Running kernel release.
    pub kernel: Option<String>,
    /// Hardware name of the running kernel (`uname -m`), not the build target:
    /// a 32-bit binary may run on a 64-bit kernel.
    pub arch: Option<String>,
    /// Seconds since boot.
    pub uptime_secs: Option<u64>,
    /// Local date and time with the time zone (`Tue 24 Nov 10:26 CET`), at
    /// [`Report::generated_at`].
    pub local_time: Option<String>,
}

/// How long one collector took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// Which collector.
    pub section: SectionId,
    /// Wall time until its answer, or until the deadline if it timed out.
    pub elapsed: Duration,
}

/// Facts about the run itself, not the system.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Meta {
    /// One entry per collector that was started.
    pub timings: Vec<Timing>,
    /// Why the config file was rejected (the defaults were used instead).
    /// Shown as an alert, so a broken file is noticed without blocking login.
    pub config_error: Option<String>,
}

/// Everything one run collected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// When the report was made, in seconds since the Unix epoch (file mode
    /// shows the data age from it).
    pub generated_at: i64,
    /// Host identity, always shown.
    pub host: Host,
    /// Sections, in collection order.
    pub sections: Vec<Section>,
    /// Run timings.
    pub meta: Meta,
}

impl Report {
    /// The worst status of all sections.
    #[must_use]
    pub fn status(&self) -> Status {
        Status::worst(self.sections.iter().map(|section| section.status))
    }

    /// The section with this id, if it was collected.
    #[must_use]
    pub fn section(&self, id: SectionId) -> Option<&Section> {
        self.sections.iter().find(|section| section.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_order_is_severity() {
        assert!(Status::Ok < Status::Unknown);
        assert!(Status::Unknown < Status::Warn);
        assert!(Status::Warn < Status::Crit);
        assert_eq!(Status::worst([]), Status::Ok);
        assert_eq!(
            Status::worst([Status::Warn, Status::Ok, Status::Unknown]),
            Status::Warn
        );
        assert_eq!(Status::worst([Status::Unknown, Status::Crit]), Status::Crit);
    }

    #[test]
    fn section_names_round_trip_and_are_unique() {
        for id in SectionId::ALL {
            assert_eq!(SectionId::from_name(id.as_str()), Some(id));
        }
        let mut names: Vec<_> = SectionId::ALL.iter().map(|id| id.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SectionId::ALL.len());
        assert_eq!(SectionId::from_name("network"), None);
        assert_eq!(SectionId::from_name(""), None);
    }

    #[test]
    fn missing_item_starts_unknown() {
        assert_eq!(Item::new("ram", Value::Missing).status, Status::Unknown);
        let usage = Value::Usage { used: 1, total: 2 };
        assert_eq!(Item::new("/", usage).status, Status::Ok);
    }

    #[test]
    fn section_status_is_worst_item() {
        let mut warn = Item::new("/home", Value::Usage { used: 9, total: 10 });
        warn.status = Status::Warn;
        let items = vec![Item::new("/", Value::Usage { used: 1, total: 10 }), warn];
        let mut section = Section::new(SectionId::Disks, items);
        assert_eq!(section.status, Status::Warn);
        assert!(section.error.is_none());

        if let Some(first) = section.items.first_mut() {
            first.status = Status::Crit;
        }
        section.refresh_status();
        assert_eq!(section.status, Status::Crit);

        assert_eq!(Section::new(SectionId::Ports, vec![]).status, Status::Ok);
    }

    #[test]
    fn failed_section_stays_unknown() {
        let mut section = Section::failed(SectionId::Services, "timeout");
        assert_eq!(section.status, Status::Unknown);
        assert_eq!(section.error, Some("timeout"));
        section.refresh_status();
        assert_eq!(section.status, Status::Unknown);
    }

    #[test]
    fn report_status_and_lookup() {
        let mut crit = Item::new("httpd", Value::State(ServiceState::Failed));
        crit.status = Status::Crit;
        let report = Report {
            sections: vec![
                Section::failed(SectionId::Load, "timeout"),
                Section::new(SectionId::Services, vec![crit]),
            ],
            ..Report::default()
        };
        assert_eq!(report.status(), Status::Crit);
        assert_eq!(
            report.section(SectionId::Load).and_then(|s| s.error),
            Some("timeout")
        );
        assert!(report.section(SectionId::Disks).is_none());
        assert_eq!(Report::default().status(), Status::Ok);
    }

    #[test]
    fn display_names() {
        assert_eq!(Status::Crit.to_string(), "crit");
        assert_eq!(SectionId::Network.to_string(), "net");
        assert_eq!(ServiceState::NotFound.to_string(), "not-found");
    }
}
