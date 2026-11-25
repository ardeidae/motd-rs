//! `/proc/self/mountinfo`: one line per mount of the current mount namespace,
//! in mount order:
//!
//! ```text
//! 30 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw
//! ^id ^parent ^major:minor ^root ^mount point ^options [optional fields] - ^type ^source ^super options
//! ```
//!
//! Unlike `/etc/fstab` or `/proc/mounts`, it lists everything actually
//! mounted (systemd mount units, autofs, bind mounts) and says which part of
//! the filesystem each mount shows (`root`). Paths escape space, tab, newline
//! and backslash as 3-digit octal (`\040`).

use super::ParseError;

const SOURCE: &str = "/proc/self/mountinfo";

/// One mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// `major:minor` of the filesystem (`8:2`; `0:N` for filesystems without
    /// a block device). Each btrfs subvolume gets its own.
    pub device: String,
    /// Directory of the filesystem shown at the mount point: `/` for a whole
    /// filesystem, a subdirectory for a bind mount or a btrfs subvolume.
    pub root: String,
    /// Where it is mounted, unescaped.
    pub mount_point: String,
    /// Filesystem type (`ext4`, `fuse.sshfs`).
    pub fs_type: String,
    /// Mount source, unescaped (`/dev/sda2`, `server:/export`, `tmpfs`).
    pub source: String,
}

/// Parses every line.
///
/// # Errors
///
/// A line with too few fields, without the `-` separator, or with a bad
/// escape. One bad line fails the whole file: the kernel never writes one,
/// so the input is not mountinfo.
pub fn parse(input: &str) -> Result<Vec<Mount>, ParseError> {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_line)
        .collect()
}

fn parse_line(line: &str) -> Result<Mount, ParseError> {
    let bad = |why: &str| ParseError::new(SOURCE, format!("{why}: '{line}'"));
    // Fields are separated by single spaces; escaped paths contain none.
    let fields: Vec<&str> = line.split(' ').collect();
    let field = |i: usize| fields.get(i).copied().filter(|f| !f.is_empty());
    let (Some(device), Some(root), Some(mount_point), Some(_options)) =
        (field(2), field(3), field(4), field(5))
    else {
        return Err(bad("fewer than 6 fields"));
    };
    if !device
        .split_once(':')
        .is_some_and(|(major, minor)| is_number(major) && is_number(minor))
    {
        return Err(bad("bad major:minor"));
    }
    // Zero or more optional fields (`shared:1`, `master:2`), then a field
    // that is exactly `-`, then the type and the source.
    let separator = fields
        .iter()
        .skip(6)
        .position(|f| *f == "-")
        .ok_or_else(|| bad("no '-' separator"))?
        + 6;
    let (Some(fs_type), Some(source)) = (field(separator + 1), field(separator + 2)) else {
        return Err(bad("no type or source"));
    };
    Ok(Mount {
        device: device.to_owned(),
        root: unescape(root).ok_or_else(|| bad("bad escape in root"))?,
        mount_point: unescape(mount_point).ok_or_else(|| bad("bad escape in mount point"))?,
        fs_type: fs_type.to_owned(),
        source: unescape(source).ok_or_else(|| bad("bad escape in source"))?,
    })
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `/media/My\040Disk` → `/media/My Disk`. The kernel escapes exactly space,
/// tab, newline and backslash, always as a backslash and 3 octal digits. A
/// lone backslash or a value above `\377` is malformed.
fn unescape(field: &str) -> Option<String> {
    if !field.contains('\\') {
        return Some(field.to_owned());
    }
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'\\' {
            let digits = bytes.get(i + 1..i + 4)?;
            let mut value: u32 = 0;
            for &d in digits {
                if !(b'0'..=b'7').contains(&d) {
                    return None;
                }
                value = value * 8 + u32::from(d - b'0');
            }
            out.push(u8::try_from(value).ok()?);
            i += 4;
        } else {
            out.push(b);
            i += 1;
        }
    }
    // An escape may produce a byte that is not valid UTF-8 on its own: such
    // a path is shown with a replacement character rather than dropped.
    Some(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(device: &str, root: &str, mount_point: &str, fs_type: &str, source: &str) -> Mount {
        Mount {
            device: device.into(),
            root: root.into(),
            mount_point: mount_point.into(),
            fs_type: fs_type.into(),
            source: source.into(),
        }
    }

    #[test]
    fn real_input() {
        // rpi4-ubuntu, cut.
        let input = "\
27 30 0:5 / /dev rw,nosuid,relatime shared:2 - devtmpfs udev rw,size=3699276k,nr_inodes=924819,mode=755,inode64
30 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw
47 30 8:1 / /boot/firmware rw,relatime shared:29 - vfat /dev/sda1 rw,fmask=0022,dmask=0022,codepage=437
37 26 0:32 / /proc/sys/fs/binfmt_misc rw,relatime shared:13 - autofs systemd-1 rw,fd=32,pgrp=1,timeout=0
";
        assert_eq!(
            parse(input),
            Ok(vec![
                mount("0:5", "/", "/dev", "devtmpfs", "udev"),
                mount("8:2", "/", "/", "ext4", "/dev/sda2"),
                mount("8:1", "/", "/boot/firmware", "vfat", "/dev/sda1"),
                mount(
                    "0:32",
                    "/",
                    "/proc/sys/fs/binfmt_misc",
                    "autofs",
                    "systemd-1"
                ),
            ])
        );
    }

    #[test]
    fn optional_fields_vary() {
        // None (container), several, and a bind mount of a file.
        let input = "\
612 586 0:52 / / rw,relatime - overlay overlay rw,lowerdir=/x
620 612 253:1 /var/lib/docker/containers/abc/hostname /etc/hostname rw,relatime shared:3 master:1 - ext4 /dev/mapper/vg1 rw
";
        let mounts = parse(input).unwrap();
        assert_eq!(mounts[0], mount("0:52", "/", "/", "overlay", "overlay"));
        assert_eq!(
            mounts[1],
            mount(
                "253:1",
                "/var/lib/docker/containers/abc/hostname",
                "/etc/hostname",
                "ext4",
                "/dev/mapper/vg1"
            )
        );
    }

    #[test]
    fn escapes_are_decoded() {
        let line = r"90 30 8:17 /a\134b /media/My\040Disk\011x rw - ext4 /dev/sd\040b1 rw";
        let [m] = parse(line).unwrap().try_into().unwrap();
        assert_eq!(m.root, r"/a\b");
        assert_eq!(m.mount_point, "/media/My Disk\tx");
        assert_eq!(m.source, "/dev/sd b1");
        // Non-ASCII names are not escaped by the kernel.
        let line = "91 30 8:18 / /media/Données rw - exfat /dev/sdc1 rw";
        assert_eq!(parse(line).unwrap()[0].mount_point, "/media/Données");
        // An escaped byte that is not UTF-8 on its own.
        let line = r"92 30 8:19 / /mnt/\377 rw - ext4 /dev/sdd1 rw";
        assert_eq!(parse(line).unwrap()[0].mount_point, "/mnt/\u{fffd}");
    }

    #[test]
    fn separator_must_be_a_whole_field() {
        // A mount point ending in "-" is not the separator.
        let line = "93 30 8:20 / /mnt/a- rw shared:1 - ext4 /dev/sde1 rw";
        assert_eq!(parse(line).unwrap()[0].mount_point, "/mnt/a-");
    }

    #[test]
    fn empty_input_is_no_mount() {
        assert_eq!(parse(""), Ok(vec![]));
        assert_eq!(parse("\n\n"), Ok(vec![]));
    }

    #[test]
    fn fixtures_parse() {
        for machine in ["rpi4-ubuntu", "container"] {
            let path = format!(
                "{}/tests/fixtures/{machine}/proc/self/mountinfo",
                env!("CARGO_MANIFEST_DIR")
            );
            let mounts = parse(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert!(mounts.iter().any(|m| m.mount_point == "/"), "{machine}");
        }
    }

    #[test]
    fn malformed_input() {
        for bad in [
            "garbage",
            "30 1 8:2 / /",
            "30 1 8:2 / / rw",
            "30 1 8:2 / / rw shared:1 ext4 /dev/sda2 rw",
            "30 1 8:2 / / rw -",
            "30 1 8:2 / / rw - ext4",
            "30 1 82 / / rw - ext4 /dev/sda2 rw",
            "30 1 8:x / / rw - ext4 /dev/sda2 rw",
            r"30 1 8:2 / /a\04 rw - ext4 /dev/sda2 rw",
            r"30 1 8:2 / /a\048 rw - ext4 /dev/sda2 rw",
            r"30 1 8:2 / /a\400 rw - ext4 /dev/sda2 rw",
            r"30 1 8:2 / /a\ rw - ext4 /dev/sda2 rw",
            // Truncated in the middle of the second line.
            "30 1 8:2 / / rw - ext4 /dev/sda2 rw\n47 30 8:1 / /boot",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
