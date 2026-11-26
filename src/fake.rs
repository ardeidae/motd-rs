//! A captured machine's faked answers, read from `<sysroot>/fake.toml`: the
//! syscalls (`[sys]`, see [`FakeSys`]), systemd (`[systemd]`, see
//! [`FakeSystemd`]) and logind (`[logind]`, see [`FakeLogind`]). Used by
//! tests and whenever `--sysroot` is given.

use std::fmt;
use std::io;
use std::path::Path;

use serde::Deserialize;

use crate::sys::{FakeSys, Sys};
use crate::systemd::{FakeBus, FakeLogind, FakeSystemd, Systemd};

/// Contents of `fake.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Fake {
    /// Syscall answers.
    pub sys: FakeSys,
    /// systemd answers. `None` (no `[systemd]` table) = a machine without
    /// systemd, like a container.
    pub systemd: Option<FakeSystemd>,
    /// logind sessions. `None` (no `[logind]` table) = no logind on the bus,
    /// so the users section falls back to utmp.
    pub logind: Option<FakeLogind>,
}

/// `fake.toml` could not be read or parsed.
#[derive(Debug)]
pub enum FakeError {
    /// I/O error other than "not found".
    Io(io::Error),
    /// Invalid TOML, wrong type or unknown key.
    Toml(toml::de::Error),
}

impl fmt::Display for FakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read fake.toml: {e}"),
            Self::Toml(e) => write!(f, "invalid fake.toml: {e}"),
        }
    }
}

impl std::error::Error for FakeError {}

impl Fake {
    /// Parses a `fake.toml` document.
    ///
    /// # Errors
    ///
    /// Invalid TOML, wrong types or unknown keys.
    pub fn from_toml(text: &str) -> Result<Self, FakeError> {
        toml::from_str(text).map_err(FakeError::Toml)
    }

    /// Loads `<root>/fake.toml`. A missing file gives an empty fake (no
    /// syscall answers, no systemd), so a bare fake root still renders.
    ///
    /// # Errors
    ///
    /// Unreadable or invalid file.
    pub fn load(root: &Path) -> Result<Self, FakeError> {
        match std::fs::read_to_string(root.join("fake.toml")) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(FakeError::Io(e)),
        }
    }

    /// Splits the fake into the two backends the collectors use.
    #[must_use]
    pub fn into_backends(self) -> (Box<dyn Sys>, Box<dyn Systemd>) {
        let bus = FakeBus {
            systemd: self.systemd,
            logind: self.logind,
        };
        (Box::new(self.sys), Box::new(bus))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_tables() {
        let fake = Fake::from_toml(
            r#"
            [sys]
            machine = "armv6l"

            [[systemd.units]]
            name = "sshd.service"
            description = "OpenSSH server"
            load_state = "loaded"
            active_state = "active"
            sub_state = "running"

            [logind]
            sessions = ["user1 user active remote"]
            "#,
        )
        .unwrap();
        assert_eq!(fake.sys.machine.as_deref(), Some("armv6l"));
        let (sys, systemd) = fake.into_backends();
        assert_eq!(sys.machine().unwrap(), "armv6l");
        let answer = systemd.query(&["sshd.service".into()], false).unwrap();
        assert_eq!(answer.watched[0].sub_state, "running");
        assert!(systemd.sessions().unwrap()[0].remote);
    }

    #[test]
    fn no_systemd_table_means_no_systemd() {
        let (_, systemd) = Fake::from_toml("[sys]\n").unwrap().into_backends();
        assert!(systemd.query(&[], true).is_err());
        assert!(systemd.sessions().is_err());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Fake::from_toml("machine = \"x\"").is_err()); // must be under [sys]
        assert!(Fake::from_toml("[systemd]\nbogus = 1").is_err());
        assert!(Fake::from_toml("[[systemd.units]]\nname = \"a.service\"").is_err());
        assert!(Fake::from_toml("[logind]\nsessions = [\"a user active\"]").is_err());
        assert!(Fake::from_toml("[logind]\nsessions = [\"a user active ssh\"]").is_err());
        assert!(Fake::from_toml("[logind]\nsessions = [\"a b c local d\"]").is_err());
        assert!(Fake::from_toml("[logind]\nbogus = 1").is_err());
        assert!(Fake::from_toml("[sys").is_err());
    }

    #[test]
    fn missing_file_is_empty() {
        let fake = Fake::load(Path::new("/nonexistent-motd-rs-root")).unwrap();
        assert_eq!(fake, Fake::default());
    }
}
