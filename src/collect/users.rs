//! Who is logged in: from logind's session files, or from utmp where there
//! is no logind.

use std::collections::BTreeSet;
use std::io;

use crate::collect::{CollectError, Collector, Ctx};
use crate::model::{Item, SectionId, Value, users_items};
use crate::parse::{session, utmp};

const SESSIONS_DIR: &str = "/run/systemd/sessions";
/// `/var/run` is a symlink to `/run` on every supported distribution.
const UTMP: &str = "/run/utmp";

/// The `users` section: [`users_items`] counts, or no item at all when the
/// system keeps no session records.
#[derive(Debug)]
pub struct Users;

impl Collector for Users {
    fn id(&self) -> SectionId {
        SectionId::Users
    }

    /// logind first: when its directory exists it is the truth, even when
    /// empty (nobody logged in). utmp is only the fallback for systems
    /// without logind. Neither: nothing to show, which is not an error.
    fn collect(&self, ctx: &Ctx) -> Result<Vec<Item>, CollectError> {
        let logins = match ctx.sysroot.read_dir_names(SESSIONS_DIR) {
            Ok(names) => logind_sessions(ctx, &names),
            Err(err) if err.kind() == io::ErrorKind::NotFound => match ctx.sysroot.read(UTMP) {
                Ok(bytes) => utmp::parse(&bytes)?
                    .into_iter()
                    .map(|login| (login.user, login.remote))
                    .collect(),
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(source) => return Err(CollectError::Io { path: UTMP, source }),
            },
            Err(source) => {
                return Err(CollectError::Io {
                    path: SESSIONS_DIR,
                    source,
                });
            }
        };
        Ok(items(&logins))
    }
}

/// `(user, remote)` of every open user session. A file that vanished since
/// the listing (logout) or does not parse is skipped: one odd session must
/// not hide the others.
fn logind_sessions(ctx: &Ctx, names: &[String]) -> Vec<(String, bool)> {
    names
        .iter()
        // `<id>.ref` entries are FIFOs that logind uses to track sessions.
        .filter(|name| !name.contains('.'))
        .filter_map(|name| {
            let text = ctx
                .sysroot
                .read_small::<4096>(&format!("{SESSIONS_DIR}/{name}"))
                .ok()?;
            let session = session::parse(&text).ok()?;
            session
                .is_open_user_session()
                .then_some((session.user, session.remote))
        })
        .collect()
}

fn items(logins: &[(String, bool)]) -> Vec<Item> {
    let users: BTreeSet<&str> = logins.iter().map(|(user, _)| user.as_str()).collect();
    let count = |n: usize| Value::Count(u64::try_from(n).unwrap_or(u64::MAX));
    vec![
        Item::new(users_items::USERS, count(users.len())),
        Item::new(users_items::SESSIONS, count(logins.len())),
        Item::new(
            users_items::REMOTE,
            count(logins.iter().filter(|(_, remote)| *remote).count()),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use crate::parse::utmp::tests::record;
    use crate::sysroot::Sysroot;
    use std::path::PathBuf;

    fn ctx(root: impl Into<PathBuf>) -> Ctx {
        let sysroot = Sysroot::new(root);
        let (sys, systemd) = Fake::load(sysroot.root())
            .unwrap_or_default()
            .into_backends();
        Ctx {
            sysroot,
            sys,
            systemd,
        }
    }

    fn counts(items: &[Item]) -> Vec<(&str, u64)> {
        items
            .iter()
            .map(|i| match i.value {
                Value::Count(n) => (i.label.as_str(), n),
                _ => panic!("{i:?}"),
            })
            .collect()
    }

    /// A temporary root; `files` are (path, bytes).
    fn root(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("motd-rs-users-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    fn collect_in(name: &str, files: &[(&str, &[u8])]) -> Result<Vec<Item>, CollectError> {
        let dir = root(name, files);
        let result = Users.collect(&ctx(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        result
    }

    #[test]
    fn rpi4_ubuntu_one_user_two_ssh_sessions() {
        let items = Users
            .collect(&ctx(format!(
                "{}/tests/fixtures/rpi4-ubuntu",
                env!("CARGO_MANIFEST_DIR")
            )))
            .unwrap();
        assert_eq!(
            counts(&items),
            [("users", 1), ("sessions", 2), ("remote", 2)]
        );
    }

    #[test]
    fn container_has_no_session_records() {
        let items = Users
            .collect(&ctx(format!(
                "{}/tests/fixtures/container",
                env!("CARGO_MANIFEST_DIR")
            )))
            .unwrap();
        assert_eq!(items, []);
    }

    #[test]
    fn logind_skips_non_people_closing_and_broken_files() {
        let items = collect_in(
            "logind",
            &[
                (
                    "run/systemd/sessions/1",
                    b"USER=alice\nCLASS=user\nSTATE=active\n",
                ),
                ("run/systemd/sessions/1.ref", b""),
                (
                    "run/systemd/sessions/2",
                    b"USER=alice\nCLASS=user\nSTATE=online\nREMOTE=1\n",
                ),
                (
                    "run/systemd/sessions/3",
                    b"USER=bob\nCLASS=user\nSTATE=closing\n",
                ),
                (
                    "run/systemd/sessions/4",
                    b"USER=gdm\nCLASS=greeter\nSTATE=online\n",
                ),
                ("run/systemd/sessions/5", b"garbage"),
                // logind wins: utmp is ignored when the directory exists.
                ("run/utmp", &record(7, "mallory", "192.0.2.9")),
            ],
        )
        .unwrap();
        assert_eq!(
            counts(&items),
            [("users", 1), ("sessions", 2), ("remote", 1)]
        );
    }

    #[test]
    fn empty_logind_directory_is_zero_not_unknown() {
        let dir = root("empty", &[]);
        std::fs::create_dir_all(dir.join("run/systemd/sessions")).unwrap();
        let items = Users.collect(&ctx(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            counts(&items),
            [("users", 0), ("sessions", 0), ("remote", 0)]
        );
    }

    #[test]
    fn utmp_fallback() {
        let utmp = [
            record(7, "alice", ""),
            record(7, "alice", "192.0.2.7"),
            record(7, "bob", "192.0.2.8"),
            record(8, "dave", ""),
        ]
        .concat();
        let items = collect_in("utmp", &[("run/utmp", &utmp)]).unwrap();
        assert_eq!(
            counts(&items),
            [("users", 2), ("sessions", 3), ("remote", 2)]
        );
    }

    #[test]
    fn malformed_utmp_fails_the_section() {
        let err = collect_in("bad-utmp", &[("run/utmp", b"short")]).unwrap_err();
        assert_eq!(err.reason(), "unparsable");
    }
}
