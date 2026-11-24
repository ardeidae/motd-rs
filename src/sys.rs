//! The only module allowed to contain `unsafe`: thin, safe
//! wrappers around the few libc calls that have no `/proc` equivalent.
//!
//! Collectors never call these functions directly: they go through the [`Sys`]
//! trait, so that tests (and `--sysroot`) can use [`FakeSys`], which reads its
//! answers from the fixture's `fake.toml`.
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::fmt;
use std::io;
use std::mem::MaybeUninit;
use std::net::Ipv4Addr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

/// Format of the header date. The process never calls
/// `setlocale`, so `strftime` runs in the "C" locale: English names, no locale
/// surprises.
const DATE_FORMAT: &CStr = c"%a %d %b %H:%M %Z";

/// Space and inode counts of a mounted filesystem, from `statvfs(3)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsStats {
    /// Size of the filesystem.
    pub total_bytes: u64,
    /// Free space, including the blocks reserved for root.
    pub free_bytes: u64,
    /// Free space usable by unprivileged users (what `df` shows as "Avail").
    pub avail_bytes: u64,
    /// Number of inodes (0 on filesystems without a fixed inode table, e.g. btrfs).
    pub total_inodes: u64,
    /// Free inodes.
    pub free_inodes: u64,
}

/// One IPv4 address of a network interface, from `getifaddrs(3)`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ipv4Interface {
    /// Interface name (`eth0`). Not sanitised: it comes from the system.
    pub name: String,
    /// The address.
    pub addr: Ipv4Addr,
    /// Prefix length derived from the netmask (`24`).
    pub prefix_len: u8,
    /// `IFF_UP` is set.
    pub up: bool,
    /// `IFF_LOOPBACK` is set.
    pub loopback: bool,
}

/// Access to the system calls the collectors need. [`RealSys`] calls libc,
/// [`FakeSys`] answers from a fixture.
pub trait Sys: Send + Sync + fmt::Debug {
    /// Space and inodes of the filesystem mounted at `mount_point`.
    ///
    /// # Errors
    ///
    /// Any `statvfs` error (`ENOENT`, `EACCES`, …).
    fn statvfs(&self, mount_point: &Path) -> io::Result<FsStats>;

    /// Every IPv4 address of every interface, in kernel order.
    ///
    /// # Errors
    ///
    /// Any `getifaddrs` error.
    fn ipv4_interfaces(&self) -> io::Result<Vec<Ipv4Interface>>;

    /// Current time in seconds since the Unix epoch.
    fn now(&self) -> i64;

    /// `epoch_secs` as local time, formatted as `Tue 24 Nov 10:26 CET`.
    /// Follows the `TZ` variable. `None` if the conversion fails.
    fn local_time(&self, epoch_secs: i64) -> Option<String>;

    /// Width of the terminal on stdout, `None` if stdout is not a terminal.
    fn term_width(&self) -> Option<u16>;

    /// Hardware name of the running kernel (`uname -m`): `x86_64`, `aarch64`,
    /// `armv7l`… It can differ from the build target, e.g. a 32-bit binary on
    /// a 64-bit kernel.
    ///
    /// # Errors
    ///
    /// Any `uname` error.
    fn machine(&self) -> io::Result<String>;
}

/// The real system, through libc.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealSys;

impl Sys for RealSys {
    fn statvfs(&self, mount_point: &Path) -> io::Result<FsStats> {
        let path = CString::new(mount_point.as_os_str().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let mut buf = MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `path` is a valid NUL-terminated string that outlives the
        // call, and `buf` points to writable memory of the right type.
        let rc = unsafe { libc::statvfs(path.as_ptr(), buf.as_mut_ptr()) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `statvfs` returned 0, so it filled the whole struct.
        let st = unsafe { buf.assume_init() };
        Ok(fs_stats(&st))
    }

    fn ipv4_interfaces(&self) -> io::Result<Vec<Ipv4Interface>> {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: `head` is a valid out-pointer. On success the list is freed
        // below with `freeifaddrs`, exactly once.
        if unsafe { libc::getifaddrs(&raw mut head) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut out = Vec::new();
        let mut cur = head;
        while !cur.is_null() {
            // SAFETY: `cur` is a non-null node of the list returned by
            // `getifaddrs`, which stays valid until `freeifaddrs`.
            let ifa = unsafe { &*cur };
            // SAFETY: same list; the address pointers are null or valid.
            if let Some(iface) = unsafe { ipv4_entry(ifa) } {
                out.push(iface);
            }
            cur = ifa.ifa_next;
        }
        // SAFETY: `head` came from a successful `getifaddrs`, and no reference
        // into the list outlives this point (entries were copied into `out`).
        unsafe { libc::freeifaddrs(head) };
        Ok(out)
    }

    fn now(&self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
            // Clock before 1970: report it as such rather than failing.
            Err(e) => i64::try_from(e.duration().as_secs()).map_or(i64::MIN, |s| -s),
        }
    }

    fn local_time(&self, epoch_secs: i64) -> Option<String> {
        let time = libc::time_t::try_from(epoch_secs).ok()?;
        let mut tm = MaybeUninit::<libc::tm>::uninit();
        // SAFETY: both pointers are valid for the call. `localtime_r` is the
        // thread-safe variant; it reads `TZ` on first use (glibc, musl).
        if unsafe { libc::localtime_r(&raw const time, tm.as_mut_ptr()) }.is_null() {
            return None;
        }
        // SAFETY: `localtime_r` returned non-null, so it filled `tm`.
        let tm = unsafe { tm.assume_init() };
        let mut buf = [0u8; 64];
        // SAFETY: `buf` is writable for `buf.len()` bytes, the format is a
        // NUL-terminated string and `tm` is initialised. `strftime` writes at
        // most `len` bytes and returns 0 if the result did not fit.
        let len = unsafe {
            libc::strftime(
                buf.as_mut_ptr().cast(),
                buf.len(),
                DATE_FORMAT.as_ptr(),
                &raw const tm,
            )
        };
        let text = buf.get(..len)?;
        (!text.is_empty()).then(|| String::from_utf8_lossy(text).into_owned())
    }

    fn term_width(&self) -> Option<u16> {
        let mut ws = MaybeUninit::<libc::winsize>::zeroed();
        // SAFETY: `TIOCGWINSZ` writes one `winsize` through the pointer, which
        // is valid and correctly typed. On a non-terminal fd it fails (ENOTTY).
        let rc = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, ws.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        // SAFETY: zero-initialised, then filled by a successful ioctl.
        let ws = unsafe { ws.assume_init() };
        (ws.ws_col > 0).then_some(ws.ws_col)
    }

    fn machine(&self) -> io::Result<String> {
        let mut uts = MaybeUninit::<libc::utsname>::uninit();
        // SAFETY: `uts` points to writable memory of the right type.
        if unsafe { libc::uname(uts.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `uname` returned 0, so it filled the whole struct.
        let uts = unsafe { uts.assume_init() };
        Ok(c_chars_to_string(&uts.machine))
    }
}

/// Converts a `statvfs` result to bytes. Multiplications saturate: a corrupt
/// answer must not wrap around to a small value.
// `fsblkcnt_t` and `c_ulong` are u32 on some 32-bit targets, u64 elsewhere.
#[allow(clippy::useless_conversion)]
fn fs_stats(st: &libc::statvfs) -> FsStats {
    // POSIX: block counts are in units of `f_frsize`; old kernels leave it 0.
    let frsize = match u64::from(st.f_frsize) {
        0 => u64::from(st.f_bsize),
        n => n,
    };
    FsStats {
        total_bytes: u64::from(st.f_blocks).saturating_mul(frsize),
        free_bytes: u64::from(st.f_bfree).saturating_mul(frsize),
        avail_bytes: u64::from(st.f_bavail).saturating_mul(frsize),
        total_inodes: u64::from(st.f_files),
        free_inodes: u64::from(st.f_ffree),
    }
}

/// Copies an `AF_INET` entry out of the `getifaddrs` list, `None` otherwise.
///
/// # Safety
///
/// `ifa` must be a node of a live `getifaddrs` list: `ifa_name` is a valid C
/// string and `ifa_addr` / `ifa_netmask` are null or point to a `sockaddr`
/// whose real type matches its `sa_family`.
unsafe fn ipv4_entry(ifa: &libc::ifaddrs) -> Option<Ipv4Interface> {
    // SAFETY: guaranteed by the caller.
    let addr = unsafe { sockaddr_ipv4(ifa.ifa_addr) }?;
    // SAFETY: guaranteed by the caller.
    let prefix_len = unsafe { sockaddr_ipv4(ifa.ifa_netmask) }.map_or(0, |mask| {
        u8::try_from(mask.to_bits().count_ones()).unwrap_or(0)
    });
    if ifa.ifa_name.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated (guaranteed by the caller).
    let name = unsafe { CStr::from_ptr(ifa.ifa_name) }
        .to_string_lossy()
        .into_owned();
    let flags = ifa.ifa_flags;
    Some(Ipv4Interface {
        name,
        addr,
        prefix_len,
        up: flags & libc::IFF_UP.cast_unsigned() != 0,
        loopback: flags & libc::IFF_LOOPBACK.cast_unsigned() != 0,
    })
}

/// Reads an IPv4 address from a `sockaddr` pointer, `None` if it is null or
/// not `AF_INET`.
///
/// # Safety
///
/// `sa` is null or points to a valid `sockaddr` whose real type matches its
/// `sa_family`.
unsafe fn sockaddr_ipv4(sa: *const libc::sockaddr) -> Option<Ipv4Addr> {
    if sa.is_null() {
        return None;
    }
    // SAFETY: non-null and valid (caller), `sa_family` is the common prefix.
    if i32::from(unsafe { (*sa).sa_family }) != libc::AF_INET {
        return None;
    }
    // SAFETY: the family is AF_INET, so the real type is `sockaddr_in`.
    // `read_unaligned` because `sockaddr` has a smaller alignment.
    let sin = unsafe { sa.cast::<libc::sockaddr_in>().read_unaligned() };
    // `s_addr` is in network byte order, i.e. the address octets in memory order.
    Some(Ipv4Addr::from(sin.sin_addr.s_addr.to_ne_bytes()))
}

/// A NUL-terminated `c_char` array (as in `utsname`) to a `String`, stopping
/// at the array end if the NUL is missing.
fn c_chars_to_string(chars: &[libc::c_char]) -> String {
    // `c_char` is i8 or u8 depending on the target: `to_ne_bytes` covers both
    // without a sign-changing cast.
    let bytes: Vec<u8> = chars
        .iter()
        .map(|c| c.to_ne_bytes()[0])
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A fake system for tests and `--sysroot`: the `[sys]` table of the
/// fixture's `fake.toml`. Anything not listed fails (`statvfs`) or is absent
/// (`None`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FakeSys {
    /// `uname -m`.
    pub machine: Option<String>,
    /// Fixed clock (epoch seconds).
    pub now: i64,
    /// Fixed `local_time` answer, whatever the epoch.
    pub local_time: Option<String>,
    /// Fixed terminal width.
    pub term_width: Option<u16>,
    /// `getifaddrs` answer.
    pub interfaces: Vec<Ipv4Interface>,
    /// `statvfs` answers, by mount point.
    pub statvfs: BTreeMap<String, FsStats>,
}

impl Sys for FakeSys {
    fn statvfs(&self, mount_point: &Path) -> io::Result<FsStats> {
        mount_point
            .to_str()
            .and_then(|p| self.statvfs.get(p))
            .copied()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn ipv4_interfaces(&self) -> io::Result<Vec<Ipv4Interface>> {
        Ok(self.interfaces.clone())
    }

    fn now(&self) -> i64 {
        self.now
    }

    fn local_time(&self, _epoch_secs: i64) -> Option<String> {
        self.local_time.clone()
    }

    fn term_width(&self) -> Option<u16> {
        self.term_width
    }

    fn machine(&self) -> io::Result<String> {
        self.machine
            .clone()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_statvfs_of_root() {
        let st = RealSys.statvfs(Path::new("/")).unwrap();
        assert!(st.total_bytes > 0);
        assert!(st.free_bytes <= st.total_bytes);
        assert!(st.avail_bytes <= st.free_bytes);
    }

    #[test]
    fn real_statvfs_errors() {
        let missing = RealSys.statvfs(Path::new("/nonexistent-motd-rs-root"));
        assert_eq!(missing.unwrap_err().kind(), io::ErrorKind::NotFound);
        let nul = RealSys.statvfs(Path::new("/a\0b"));
        assert_eq!(nul.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn real_interfaces_include_loopback() {
        let ifaces = RealSys.ipv4_interfaces().unwrap();
        // Build sandboxes may have no network namespace with `lo` configured,
        // so only check what is returned is consistent.
        for iface in &ifaces {
            assert_ne!(iface.name, "");
            assert!(iface.prefix_len <= 32);
            if iface.loopback {
                assert!(iface.addr.is_loopback());
            }
        }
    }

    #[test]
    fn real_machine_and_time() {
        let machine = RealSys.machine().unwrap();
        assert_ne!(machine, "");
        assert!(machine.is_ascii());
        assert!(RealSys.now() > 1_700_000_000);
        let text = RealSys.local_time(RealSys.now()).unwrap();
        // "Tue 24 Nov 10:26 CET": weekday, day, month, time, zone.
        assert!(text.split(' ').count() >= 5, "{text}");
        // Test runs capture stdout: not a terminal.
        let _ = RealSys.term_width();
    }

    #[test]
    fn local_time_follows_epoch_and_c_locale() {
        // The test process may have any TZ (UTC-12..UTC+14): at 12:00 UTC this
        // is Wednesday 15 or Thursday 16. The names are English ("C" locale).
        let text = RealSys.local_time(1_700_049_600).unwrap(); // 2023-11-15 12:00 UTC
        assert!(
            text.starts_with("Wed 15 Nov ") || text.starts_with("Thu 16 Nov "),
            "{text}"
        );
        assert_eq!(RealSys.local_time(i64::MAX), None);
    }

    #[test]
    fn c_chars_stop_at_nul_or_end() {
        let chars: Vec<libc::c_char> = b"x86_64\0junk"
            .iter()
            .map(|&b| libc::c_char::from_ne_bytes([b]))
            .collect();
        assert_eq!(c_chars_to_string(&chars), "x86_64");
        let no_nul: Vec<libc::c_char> = b"arm"
            .iter()
            .map(|&b| libc::c_char::from_ne_bytes([b]))
            .collect();
        assert_eq!(c_chars_to_string(&no_nul), "arm");
    }

    #[test]
    fn fake_from_toml() {
        let fake = toml::from_str::<FakeSys>(
            r#"
            machine = "armv6l"
            now = 1795000000
            local_time = "Tue 24 Nov 10:26 CET"

            [[interfaces]]
            name = "eth0"
            addr = "192.0.2.4"
            prefix_len = 24
            up = true
            loopback = false

            [statvfs."/"]
            total_bytes = 1000
            free_bytes = 400
            avail_bytes = 300
            total_inodes = 10
            free_inodes = 5
            "#,
        )
        .unwrap();
        assert_eq!(fake.machine().unwrap(), "armv6l");
        assert_eq!(fake.now(), 1_795_000_000);
        assert_eq!(fake.local_time(0).as_deref(), Some("Tue 24 Nov 10:26 CET"));
        assert_eq!(fake.term_width(), None);
        assert_eq!(
            fake.ipv4_interfaces().unwrap()[0].addr,
            Ipv4Addr::new(192, 0, 2, 4)
        );
        assert_eq!(fake.statvfs(Path::new("/")).unwrap().avail_bytes, 300);
        assert_eq!(
            fake.statvfs(Path::new("/home")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn fake_rejects_bad_input() {
        assert!(toml::from_str::<FakeSys>("bogus = 1").is_err());
        assert!(toml::from_str::<FakeSys>("machine = 3").is_err());
        assert!(toml::from_str::<FakeSys>("[[interfaces]]\nname = \"x\"").is_err());
        assert!(toml::from_str::<FakeSys>("machine = \"x").is_err());
    }
}
