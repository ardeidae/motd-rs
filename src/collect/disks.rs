//! Space used on mounted filesystems: which mounts are worth a row
//! (from `/proc/self/mountinfo`), then `statvfs` on each.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use crate::collect::{CollectError, Collector, Ctx};
use crate::config;
use crate::glob;
use crate::model::{Item, SectionId, Value};
use crate::parse::mountinfo::{self, Mount};
use crate::sys::FsStats;

/// Filesystems that hold data on a local block device. An allowlist rather
/// than a list of pseudo filesystems to hide: a new virtual filesystem type
/// in a future kernel must not appear as a disk.
const LOCAL_TYPES: &[&str] = &[
    "bcachefs", "btrfs", "exfat", "ext2", "ext3", "ext4", "f2fs", "fuseblk", "hfs", "hfsplus",
    "jfs", "msdos", "nilfs2", "ntfs", "ntfs3", "reiserfs", "vfat", "xfs", "zfs",
];

/// Filesystems served over the network. `statvfs` on one whose server is
/// gone can sleep in the kernel for minutes, so they are opt-in.
const NETWORK_TYPES: &[&str] = &[
    "9p",
    "afs",
    "beegfs",
    "ceph",
    "cifs",
    "davfs",
    "fuse.glusterfs",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.sshfs",
    "glusterfs",
    "gpfs",
    "lustre",
    "nfs",
    "nfs4",
    "smb3",
    "smbfs",
    "sshfs",
];

/// The `disks` section: one [`Value::Usage`] item per filesystem, labelled
/// with its mount point, sorted by mount point.
#[derive(Debug)]
pub struct Disks {
    exclude: Vec<String>,
    include_network: bool,
}

impl Disks {
    /// A collector for the `[disks]` settings.
    #[must_use]
    pub fn new(config: &config::Disks) -> Self {
        Self {
            exclude: config.exclude.clone(),
            include_network: config.include_network,
        }
    }
}

impl Collector for Disks {
    fn id(&self) -> SectionId {
        SectionId::Disks
    }

    fn collect(&self, ctx: &Ctx) -> Result<Vec<Item>, CollectError> {
        let path = "/proc/self/mountinfo";
        let text = ctx
            .sysroot
            .read_lossy(path)
            .map_err(|source| CollectError::Io { path, source })?;
        let mounts = select(
            mountinfo::parse(&text)?,
            &self.exclude,
            self.include_network,
        );
        Ok(mounts
            .into_iter()
            .filter_map(|mount| {
                let value = usage(ctx.sys.statvfs(Path::new(&mount.mount_point)))?;
                Some(Item::new(mount.mount_point, value))
            })
            .collect())
    }
}

/// The value of a row, `None` to hide it.
///
/// Used and total follow `df`: used = size − free, and the percentage is
/// taken over used + available, so the blocks reserved for root (5 % on
/// ext4 by default) count as unusable. A disk shown at 100 % is full for
/// ordinary users even if root could still write.
///
/// Hidden: a mount point that vanished since mountinfo was read (`ENOENT`),
/// one the user may not look into (`EACCES`: kubelet volumes, other users'
/// FUSE mounts), and filesystems with nothing to fill (size 0). Any other
/// error (`ENOTCONN` from a dead FUSE daemon, `EIO`) is a problem worth a `?`.
fn usage(stats: io::Result<FsStats>) -> Option<Value> {
    match stats {
        Ok(stats) => {
            let used = stats.total_bytes.saturating_sub(stats.free_bytes);
            let total = used.saturating_add(stats.avail_bytes);
            (total > 0).then_some(Value::Usage { used, total })
        }
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            None
        }
        Err(_) => Some(Value::Missing),
    }
}

/// Keeps one mount per filesystem worth showing, sorted by mount point.
fn select(mounts: Vec<Mount>, exclude: &[String], include_network: bool) -> Vec<Mount> {
    // A later mount on the same mount point hides the earlier one.
    let mut last: HashMap<String, usize> = HashMap::new();
    for (index, mount) in mounts.iter().enumerate() {
        last.insert(mount.mount_point.clone(), index);
    }
    let visible = mounts
        .into_iter()
        .enumerate()
        .filter(|(index, mount)| last.get(&mount.mount_point) == Some(index))
        .map(|(_, mount)| mount)
        .filter(|mount| wanted(mount, include_network))
        .filter(|mount| !exclude.iter().any(|p| glob::matches(p, &mount.mount_point)));

    // One row per filesystem: bind mounts and btrfs subvolumes of one device
    // show the same space. The shortest mount point names it (`/` rather
    // than a kubelet path).
    let mut by_fs: HashMap<String, Mount> = HashMap::new();
    for mount in visible {
        let key = if mount.source.starts_with("/dev/") {
            mount.source.clone()
        } else {
            mount.device.clone()
        };
        match by_fs.get(&key) {
            Some(kept) if !shorter(&mount.mount_point, &kept.mount_point) => {}
            _ => {
                by_fs.insert(key, mount);
            }
        }
    }
    let mut kept: Vec<Mount> = by_fs.into_values().collect();
    kept.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    kept
}

/// Whether `a` should name a filesystem rather than `b`: fewer characters,
/// then alphabetical, so the choice does not depend on mount order.
fn shorter(a: &str, b: &str) -> bool {
    (a.chars().count(), a) < (b.chars().count(), b)
}

/// Whether a mount can be a row at all.
///
/// - Local block filesystems, plus `overlay` mounted at `/`: the root of a
///   container. Other overlays are the root filesystems of containers seen
///   from the host (one per running container on a Kubernetes node).
/// - Network filesystems only when asked for.
/// - Only mounts of a whole filesystem (`root` = `/`). A mount of a
///   subdirectory is a bind mount, a view of a filesystem mounted elsewhere
///   or of the host's disk in a container (Docker binds `/etc/hosts` from
///   it). btrfs is the exception: its subvolumes (`/@`, `/@home`) are
///   mounted that way by design.
fn wanted(mount: &Mount, include_network: bool) -> bool {
    let fs_type = mount.fs_type.as_str();
    let kind_ok = LOCAL_TYPES.contains(&fs_type)
        || (fs_type == "overlay" && mount.mount_point == "/")
        || (include_network && NETWORK_TYPES.contains(&fs_type));
    kind_ok && (mount.root == "/" || fs_type == "btrfs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use crate::sysroot::Sysroot;

    fn mount(device: &str, root: &str, mount_point: &str, fs_type: &str, source: &str) -> Mount {
        Mount {
            device: device.into(),
            root: root.into(),
            mount_point: mount_point.into(),
            fs_type: fs_type.into(),
            source: source.into(),
        }
    }

    fn points(mounts: &[Mount]) -> Vec<&str> {
        mounts.iter().map(|m| m.mount_point.as_str()).collect()
    }

    fn defaults() -> Disks {
        Disks::new(&config::Disks::default())
    }

    fn collect_fixture(name: &str, disks: &Disks) -> Vec<(String, Value)> {
        let sysroot = Sysroot::new(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ));
        let (sys, systemd) = Fake::load(sysroot.root()).unwrap().into_backends();
        let ctx = Ctx {
            sysroot,
            sys,
            systemd,
        };
        disks
            .collect(&ctx)
            .unwrap()
            .into_iter()
            .map(|i| (i.label, i.value))
            .collect()
    }

    #[test]
    fn rpi4_ubuntu() {
        // ~150 mounts: tmpfs, containerd overlays, kubelet bind mounts of
        // /dev/sda2, Longhorn volumes the capturing user could not stat.
        let rows = collect_fixture("rpi4-ubuntu", &defaults());
        let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["/", "/boot/firmware"]);
        assert!(rows.iter().all(|(_, v)| matches!(v, Value::Usage { .. })));
    }

    #[test]
    fn container_shows_its_overlay_root_only() {
        let rows = collect_fixture("container", &defaults());
        let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["/"], "no /etc/hosts bind mount of the host disk");
    }

    #[test]
    fn usage_follows_df() {
        // Size 100, 10 free of which 5 reserved for root: df says 90 used,
        // 5 available, 90 / 95 = 94.7 % (df rounds up to 95 %).
        let stats = FsStats {
            total_bytes: 100,
            free_bytes: 10,
            avail_bytes: 5,
            total_inodes: 0,
            free_inodes: 0,
        };
        assert_eq!(
            usage(Ok(stats)),
            Some(Value::Usage {
                used: 90,
                total: 95
            })
        );
        assert_eq!(usage(Ok(FsStats::default())), None, "size 0");
        let broken = FsStats {
            total_bytes: 1,
            free_bytes: 5,
            avail_bytes: u64::MAX,
            ..FsStats::default()
        };
        assert_eq!(
            usage(Ok(broken)),
            Some(Value::Usage {
                used: 0,
                total: u64::MAX
            })
        );
    }

    #[test]
    fn statvfs_errors() {
        let err = |kind| usage(Err(io::Error::from(kind)));
        assert_eq!(err(io::ErrorKind::NotFound), None);
        assert_eq!(err(io::ErrorKind::PermissionDenied), None);
        assert_eq!(err(io::ErrorKind::NotConnected), Some(Value::Missing));
        assert_eq!(
            usage(Err(io::Error::from_raw_os_error(libc::EIO))),
            Some(Value::Missing)
        );
    }

    #[test]
    fn pseudo_filesystems_are_hidden() {
        let mounts = vec![
            mount("0:5", "/", "/dev", "devtmpfs", "udev"),
            mount("0:26", "/", "/run", "tmpfs", "tmpfs"),
            mount("7:0", "/", "/snapd/core", "squashfs", "/dev/loop0"),
            mount("0:60", "/", "/run/k3s/x/rootfs", "overlay", "overlay"),
            mount("0:99", "/", "/sys/fs/newfs", "brandnewfs", "none"),
            mount("8:2", "/", "/", "ext4", "/dev/sda2"),
        ];
        assert_eq!(points(&select(mounts, &[], false)), ["/"]);
    }

    #[test]
    fn network_filesystems_are_opt_in() {
        let mounts = || {
            vec![
                mount("8:2", "/", "/", "ext4", "/dev/sda2"),
                mount("0:70", "/", "/mnt/nfs", "nfs4", "server:/export"),
                mount("0:71", "/", "/mnt/sshfs", "fuse.sshfs", "user@host:"),
            ]
        };
        assert_eq!(points(&select(mounts(), &[], false)), ["/"]);
        assert_eq!(
            points(&select(mounts(), &[], true)),
            ["/", "/mnt/nfs", "/mnt/sshfs"]
        );
    }

    #[test]
    fn excluded_mount_points() {
        let mounts = vec![
            mount("8:2", "/", "/", "ext4", "/dev/sda2"),
            mount("8:1", "/", "/boot", "ext4", "/dev/sda1"),
            mount("8:3", "/", "/boot/efi", "vfat", "/dev/sda3"),
            mount("8:4", "/", "/snap/x/1", "ext4", "/dev/sda4"),
            mount("8:5", "/", "/srv", "xfs", "/dev/sda5"),
        ];
        let exclude = config::Disks::default().exclude;
        assert_eq!(points(&select(mounts, &exclude, false)), ["/", "/srv"]);
    }

    #[test]
    fn one_row_per_filesystem() {
        let mounts = vec![
            mount(
                "8:2",
                "/var/lib/kubelet/x",
                "/var/lib/kubelet/pods/a",
                "ext4",
                "/dev/sda2",
            ),
            mount("8:2", "/", "/", "ext4", "/dev/sda2"),
            mount(
                "8:17",
                "/",
                "/var/lib/kubelet/plugins/pvc/globalmount",
                "ext4",
                "/dev/sdb1",
            ),
            mount(
                "8:17",
                "/",
                "/var/lib/kubelet/pods/b/mount",
                "ext4",
                "/dev/sdb1",
            ),
        ];
        assert_eq!(
            points(&select(mounts, &[], false)),
            ["/", "/var/lib/kubelet/pods/b/mount"]
        );
    }

    #[test]
    fn btrfs_subvolumes_share_one_row() {
        // Each subvolume has its own major:minor but the same device.
        let mounts = vec![
            mount("0:31", "/@", "/", "btrfs", "/dev/nvme0n1p2"),
            mount("0:32", "/@home", "/home", "btrfs", "/dev/nvme0n1p2"),
            mount("0:33", "/@data", "/data", "btrfs", "/dev/sdb1"),
        ];
        assert_eq!(points(&select(mounts, &[], false)), ["/", "/data"]);
    }

    #[test]
    fn zfs_datasets_are_rows() {
        // No block device: each dataset is its own filesystem.
        let mounts = vec![
            mount("0:40", "/", "/", "zfs", "rpool/ROOT/ubuntu"),
            mount("0:41", "/", "/home", "zfs", "rpool/USERDATA"),
        ];
        assert_eq!(points(&select(mounts, &[], false)), ["/", "/home"]);
    }

    #[test]
    fn last_mount_on_a_point_wins() {
        let mounts = vec![
            mount("8:2", "/", "/mnt", "ext4", "/dev/sda2"),
            mount("0:50", "/", "/mnt", "tmpfs", "tmpfs"),
        ];
        assert!(select(mounts, &[], false).is_empty(), "/mnt is a tmpfs now");
    }

    #[test]
    fn unreadable_or_malformed_mountinfo_fails_the_section() {
        let ctx = |root: Sysroot| {
            let (sys, systemd) = Fake::default().into_backends();
            Ctx {
                sysroot: root,
                sys,
                systemd,
            }
        };
        let err = defaults()
            .collect(&ctx(Sysroot::new("/nonexistent-motd-rs-root")))
            .unwrap_err();
        assert_eq!(err.reason(), "unreadable");

        let dir = std::env::temp_dir().join(format!("motd-rs-disks-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("proc/self")).unwrap();
        std::fs::write(dir.join("proc/self/mountinfo"), "garbage\n").unwrap();
        let err = defaults().collect(&ctx(Sysroot::new(&dir))).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(err.reason(), "unparsable");
    }
}
