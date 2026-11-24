//! Where the report goes: stdout, or a file replaced atomically.
//!
//! A file written in place can be read half-written by someone logging in at
//! that moment, and a crash mid-write leaves it truncated. Writing a temporary
//! file next to the target and renaming it over the target avoids both:
//! `rename(2)` within one filesystem is atomic, so a reader sees either the
//! old report or the new one.

use std::ffi::OsString;
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Mode of the written file: the MOTD is read by every user who logs in.
const MODE: u32 = 0o644;

/// Writes `bytes` to stdout.
///
/// # Errors
///
/// stdout is closed or full (e.g. `motd-rs | head -0`).
pub fn to_stdout(bytes: &[u8]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(bytes)?;
    stdout.flush()
}

/// Replaces `path` with `bytes` atomically, with mode 0644.
///
/// If `path` is a symbolic link, the link itself is replaced, never the file it
/// points to: a link planted in a shared directory cannot redirect the write.
///
/// # Errors
///
/// `path` has no file name, or the temporary file cannot be created, written
/// or renamed (missing directory, no permission, disk full…). The target is
/// then left untouched and the temporary file is removed.
pub fn to_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = temp_path(path)?;
    // A temporary file left by a crashed run with the same pid would make
    // `create_new` fail forever.
    match fs::remove_file(&temp) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let result = write_new(&temp, bytes).and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        // Best effort: the original error is the one worth reporting.
        let _ = fs::remove_file(&temp);
    }
    result
}

/// `.<name>.motd-rs-<pid>.tmp` in the target's directory: the same
/// filesystem, so the rename is atomic; hidden, so a `*.d` directory reader
/// that skips dot files never shows it.
fn temp_path(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the output path has no file name",
        )
    })?;
    let mut temp_name = OsString::from(".");
    temp_name.push(name);
    temp_name.push(format!(".motd-rs-{}.tmp", std::process::id()));
    Ok(path.with_file_name(temp_name))
}

fn write_new(temp: &Path, bytes: &[u8]) -> io::Result<()> {
    // `create_new` is O_EXCL: it never follows a symbolic link.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(MODE)
        .open(temp)?;
    // The umask may have removed bits from the mode above.
    file.set_permissions(Permissions::from_mode(MODE))?;
    file.write_all(bytes)?;
    // Without it a power loss right after the rename can leave an empty file.
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("motd-rs-output-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &Path) -> Vec<OsString> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn creates_then_replaces_with_mode_0644() {
        let dir = scratch("replace");
        let target = dir.join("50-motd-rs");
        to_file(&target, b"first\n").unwrap();
        to_file(&target, b"second\n").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second\n");
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, MODE);
        assert_eq!(entries(&dir), ["50-motd-rs"], "no temporary file left");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_temp_file_is_replaced() {
        let dir = scratch("stale");
        let target = dir.join("motd");
        let temp = temp_path(&target).unwrap();
        fs::write(&temp, "stale").unwrap();
        to_file(&target, b"new\n").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new\n");
        assert!(!temp.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn symlink_is_replaced_not_followed() {
        let dir = scratch("symlink");
        let victim = dir.join("victim");
        fs::write(&victim, "keep").unwrap();
        let target = dir.join("motd");
        symlink(&victim, &target).unwrap();
        to_file(&target, b"report\n").unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
        assert!(!fs::symlink_metadata(&target).unwrap().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"report\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_rename_keeps_target_and_cleans_up() {
        let dir = scratch("rename");
        // rename(2) cannot replace a non-empty directory with a file.
        let target = dir.join("motd");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside"), "x").unwrap();
        assert!(to_file(&target, b"report\n").is_err());
        assert!(target.is_dir());
        assert_eq!(entries(&dir), ["motd"], "temporary file removed");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_directory_is_an_error() {
        let dir = scratch("missing");
        let err = to_file(&dir.join("nope/motd"), b"x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn path_without_file_name_is_an_error() {
        let err = to_file(Path::new("/"), b"x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn temp_name_is_hidden_and_next_to_the_target() {
        let temp = temp_path(Path::new("/run/motd.d/50-motd-rs")).unwrap();
        assert_eq!(temp.parent(), Some(Path::new("/run/motd.d")));
        let name = temp.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(".50-motd-rs.motd-rs-"), "{name}");
        assert_eq!(temp.extension(), Some(OsString::from("tmp").as_os_str()));
        let bare = temp_path(Path::new("motd")).unwrap();
        assert_eq!(bare.parent(), Some(Path::new("")));
    }
}
