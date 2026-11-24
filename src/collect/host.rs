//! Host identity: hostname, OS, kernel, architecture, uptime.

use crate::parse::{os_release, uptime};
use crate::sys::Sys;
use crate::sysroot::Sysroot;

/// Raw host facts. `None` = could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    /// Hardware name of the running kernel (`uname -m`), not the build target:
    /// a 32-bit binary may run on a 64-bit kernel.
    pub arch: Option<String>,
    pub uptime_secs: Option<u64>,
}

/// Reads the host facts.
#[must_use]
pub fn collect(sysroot: &Sysroot, sys: &dyn Sys) -> Host {
    Host {
        hostname: read_trimmed(sysroot, "/proc/sys/kernel/hostname"),
        os: ["/etc/os-release", "/usr/lib/os-release"]
            .iter()
            .find_map(|path| sysroot.read_to_string(path).ok())
            .and_then(|content| os_release::parse(&content).display_name()),
        kernel: read_trimmed(sysroot, "/proc/sys/kernel/osrelease"),
        arch: sys.machine().ok().filter(|m| !m.is_empty()),
        uptime_secs: sysroot
            .read_to_string("/proc/uptime")
            .ok()
            .and_then(|content| uptime::parse(&content).ok()),
    }
}

fn read_trimmed(sysroot: &Sysroot, path: &str) -> Option<String> {
    let content = sysroot.read_to_string(path).ok()?;
    let trimmed = content.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::FakeSys;

    fn fixture(name: &str) -> Sysroot {
        Sysroot::new(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    #[test]
    fn reads_fixture() {
        let root = fixture("minimal");
        let host = collect(&root, &FakeSys::load(root.root()).unwrap());
        assert_eq!(host.hostname.as_deref(), Some("raspberrypi"));
        assert_eq!(host.os.as_deref(), Some("Debian GNU/Linux 13 (trixie)"));
        assert_eq!(host.kernel.as_deref(), Some("6.12.20-v8+"));
        assert_eq!(host.arch.as_deref(), Some("aarch64"));
        assert_eq!(host.uptime_secs, Some(1_642_000));
    }

    #[test]
    fn empty_root_gives_unknowns() {
        let host = collect(
            &Sysroot::new("/nonexistent-motd-rs-root"),
            &FakeSys::default(),
        );
        assert_eq!(host.hostname, None);
        assert_eq!(host.os, None);
        assert_eq!(host.kernel, None);
        assert_eq!(host.arch, None);
        assert_eq!(host.uptime_secs, None);
    }
}
