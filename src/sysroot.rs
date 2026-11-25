//! Every file read goes through a [`Sysroot`], so that tests can point the
//! whole program at a captured machine (`tests/fixtures/<machine>/`).

use std::io;
use std::path::{Path, PathBuf};

/// A root directory under which absolute system paths are resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sysroot {
    root: PathBuf,
}

impl Default for Sysroot {
    fn default() -> Self {
        Self::new("/")
    }
}

impl Sysroot {
    /// Creates a sysroot at `root` (`/` for the real machine).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolves an absolute system path (`/proc/uptime`) under the root.
    #[must_use]
    pub fn path(&self, system_path: &str) -> PathBuf {
        self.root.join(system_path.trim_start_matches('/'))
    }

    /// Reads a whole file as UTF-8.
    ///
    /// # Errors
    ///
    /// Any I/O error, including invalid UTF-8.
    pub fn read_to_string(&self, system_path: &str) -> io::Result<String> {
        std::fs::read_to_string(self.path(system_path))
    }

    /// Reads a whole file; invalid UTF-8 is replaced instead of failing. For
    /// files that may carry raw non-UTF-8 bytes from users, such as mount
    /// points in mountinfo: one odd name must not hide every other line.
    ///
    /// # Errors
    ///
    /// The file cannot be opened or read.
    pub fn read_lossy(&self, system_path: &str) -> io::Result<String> {
        let bytes = std::fs::read(self.path(system_path))?;
        Ok(String::from_utf8(bytes)
            .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned()))
    }

    /// Reads a small file (at most `N` bytes) with one buffer and no size
    /// query: `/proc` files report a size of 0, so `read_to_string` would ask
    /// for the size, then read in small growing chunks. Scanning
    /// `/proc/<pid>/stat` for every process makes that overhead matter.
    /// Content past `N` bytes is ignored; invalid UTF-8 is replaced.
    ///
    /// # Errors
    ///
    /// The file cannot be opened or read.
    pub fn read_small<const N: usize>(&self, system_path: &str) -> io::Result<String> {
        use std::io::Read;
        let mut file = std::fs::File::open(self.path(system_path))?;
        let mut buf = [0_u8; N];
        let mut len = 0;
        // A short read is not the end of the file in general: read until 0.
        while let Some(rest) = buf.get_mut(len..) {
            if rest.is_empty() {
                break;
            }
            match file.read(rest) {
                Ok(0) => break,
                Ok(n) => len += n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
        Ok(String::from_utf8_lossy(buf.get(..len).unwrap_or_default()).into_owned())
    }

    /// Names of the entries of a directory (not `.` and `..`), in no
    /// particular order. Names that are not valid UTF-8 are skipped: no system
    /// directory we list (`/proc`, `/lib/modules`) uses them.
    ///
    /// # Errors
    ///
    /// The directory cannot be opened. An entry that vanishes while listing
    /// (a process that exits) is skipped, not an error.
    pub fn read_dir_names(&self, system_path: &str) -> io::Result<Vec<String>> {
        Ok(std::fs::read_dir(self.path(system_path))?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect())
    }

    /// The root directory itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_absolute_paths_under_root() {
        let sysroot = Sysroot::new("/fixtures/pi");
        assert_eq!(
            sysroot.path("/proc/uptime"),
            PathBuf::from("/fixtures/pi/proc/uptime")
        );
        assert_eq!(
            Sysroot::default().path("/proc/uptime"),
            PathBuf::from("/proc/uptime")
        );
    }

    #[test]
    fn read_small_reads_whole_or_cuts() {
        let sysroot = Sysroot::new(format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR")));
        let full = sysroot
            .read_small::<512>("/rpi4-ubuntu/proc/loadavg")
            .unwrap();
        assert_eq!(full, "1.56 2.67 3.05 9/1264 1949508\n");
        let cut = sysroot
            .read_small::<4>("/rpi4-ubuntu/proc/loadavg")
            .unwrap();
        assert_eq!(cut, "1.56");
        assert!(sysroot.read_small::<8>("/nonexistent").is_err());
    }

    #[test]
    fn lists_directory_names() {
        let sysroot = Sysroot::new(format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR")));
        let mut names = sysroot.read_dir_names("/rpi4-ubuntu/lib/modules").unwrap();
        names.sort();
        assert_eq!(
            names,
            ["6.8.0-1047-raspi", "6.8.0-1057-raspi", "6.8.0-1068-raspi"]
        );
        assert!(sysroot.read_dir_names("/nonexistent").is_err());
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let sysroot = Sysroot::new("/nonexistent-motd-rs-root");
        assert!(sysroot.read_to_string("/proc/uptime").is_err());
    }
}
