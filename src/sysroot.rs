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
    fn missing_file_is_an_error_not_a_panic() {
        let sysroot = Sysroot::new("/nonexistent-motd-rs-root");
        assert!(sysroot.read_to_string("/proc/uptime").is_err());
    }
}
