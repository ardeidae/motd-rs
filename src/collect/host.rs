//! Host identity: hostname, OS, kernel, architecture, uptime, local time.

use crate::model::Host;
use crate::parse::{os_release, uptime};
use crate::sys::Sys;
use crate::sysroot::Sysroot;

/// Reads the host facts. `now` (epoch seconds) is the report's timestamp, so
/// the date shown is the moment the report claims to describe.
#[must_use]
pub fn collect(sysroot: &Sysroot, sys: &dyn Sys, now: i64) -> Host {
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
        local_time: sys.local_time(now).filter(|t| !t.is_empty()),
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
    use crate::fake::Fake;

    fn fixture(name: &str) -> Sysroot {
        Sysroot::new(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    fn collect_fixture(name: &str) -> Host {
        let root = fixture(name);
        let (sys, _) = Fake::load(root.root()).unwrap().into_backends();
        collect(&root, sys.as_ref(), sys.now())
    }

    #[test]
    fn reads_fixture() {
        let host = collect_fixture("minimal");
        assert_eq!(host.hostname.as_deref(), Some("raspberrypi"));
        assert_eq!(host.os.as_deref(), Some("Debian GNU/Linux 13 (trixie)"));
        assert_eq!(host.kernel.as_deref(), Some("6.12.20-v8+"));
        assert_eq!(host.arch.as_deref(), Some("aarch64"));
        assert_eq!(host.uptime_secs, Some(1_642_000));
        assert_eq!(host.local_time, None, "no local_time in its fake.toml");
    }

    #[test]
    fn reads_captured_machines() {
        let pi = collect_fixture("rpi4-ubuntu");
        assert_eq!(pi.hostname.as_deref(), Some("rpi4-ubuntu"));
        assert_eq!(pi.os.as_deref(), Some("Ubuntu 24.04.4 LTS"));
        assert_eq!(pi.kernel.as_deref(), Some("6.8.0-1057-raspi"));
        assert_eq!(pi.arch.as_deref(), Some("aarch64"));
        assert_eq!(pi.uptime_secs, Some(13_715_221));
        assert_eq!(pi.local_time.as_deref(), Some("Wed 25 Nov 14:01 CET"));

        let container = collect_fixture("container");
        assert_eq!(container.hostname.as_deref(), Some("container"));
        assert_eq!(
            container.os.as_deref(),
            Some("Debian GNU/Linux 11 (bullseye)")
        );
        assert_eq!(container.arch.as_deref(), Some("x86_64"));
        assert_eq!(
            container.local_time.as_deref(),
            Some("Wed 25 Nov 12:35 UTC")
        );
    }

    #[test]
    fn empty_root_gives_unknowns() {
        let host = collect(
            &Sysroot::new("/nonexistent-motd-rs-root"),
            &crate::sys::FakeSys::default(),
            0,
        );
        assert_eq!(host.hostname, None);
        assert_eq!(host.os, None);
        assert_eq!(host.kernel, None);
        assert_eq!(host.arch, None);
        assert_eq!(host.uptime_secs, None);
        assert_eq!(host.local_time, None);
    }
}
