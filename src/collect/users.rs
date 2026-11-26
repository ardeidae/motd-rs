//! Who is logged in: from logind over D-Bus, or from utmp where there is
//! no logind.

use std::collections::BTreeSet;
use std::io;

use crate::collect::{CollectError, Collector, Ctx};
use crate::model::{Item, SectionId, Value, users_items};
use crate::parse::utmp;
use crate::systemd::{Session, SystemdError};

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

    /// logind first: when it answers it is the truth, even with no session
    /// (nobody logged in). Its session files under `/run/systemd/sessions`
    /// are not read: systemd marks them private, with no format promise.
    /// utmp is only the fallback for systems without logind. Neither:
    /// nothing to show, which is not an error.
    fn collect(&self, ctx: &Ctx) -> Result<Vec<Item>, CollectError> {
        let logins: Vec<(String, bool)> = match ctx.systemd.sessions() {
            Ok(sessions) => sessions
                .into_iter()
                .filter(Session::is_open_user_session)
                .map(|session| (session.user, session.remote))
                .collect(),
            Err(SystemdError::Unavailable(_)) => match ctx.sysroot.read(UTMP) {
                Ok(bytes) => utmp::parse(&bytes)?
                    .into_iter()
                    .map(|login| (login.user, login.remote))
                    .collect(),
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(source) => return Err(CollectError::Io { path: UTMP, source }),
            },
            // logind is there but failed: `?`, rather than switching to utmp,
            // which may count differently (not every login writes utmp).
            Err(err @ SystemdError::Call(_)) => return Err(CollectError::Bus(err)),
        };
        Ok(items(&logins))
    }
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

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    /// A bus that answers `sessions` with `answer`.
    #[derive(Debug)]
    struct Logind(Result<Vec<Session>, SystemdError>);

    impl crate::systemd::Systemd for Logind {
        fn query(
            &self,
            _: &[String],
            _: bool,
        ) -> Result<crate::systemd::UnitsAnswer, SystemdError> {
            Err(SystemdError::Unavailable("test".into()))
        }
        fn sessions(&self) -> Result<Vec<Session>, SystemdError> {
            self.0.clone()
        }
    }

    fn collect_with(
        name: &str,
        files: &[(&str, &[u8])],
        bus: Logind,
    ) -> Result<Vec<Item>, CollectError> {
        let dir = root(name, files);
        let mut ctx = ctx(&dir);
        ctx.systemd = Box::new(bus);
        let result = Users.collect(&ctx);
        std::fs::remove_dir_all(&dir).unwrap();
        result
    }

    fn session(user: &str, class: &str, state: &str, remote: bool) -> Session {
        Session {
            user: user.into(),
            class: class.into(),
            state: state.into(),
            remote,
        }
    }

    #[test]
    fn rpi4_ubuntu_one_user_two_ssh_sessions() {
        let items = Users.collect(&ctx(fixture("rpi4-ubuntu"))).unwrap();
        assert_eq!(
            counts(&items),
            [("users", 1), ("sessions", 2), ("remote", 2)]
        );
    }

    #[test]
    fn rpi5_desktop_console_and_ssh() {
        let items = Users.collect(&ctx(fixture("rpi5"))).unwrap();
        assert_eq!(
            counts(&items),
            [("users", 1), ("sessions", 3), ("remote", 1)]
        );
    }

    #[test]
    fn container_has_no_session_records() {
        let items = Users.collect(&ctx(fixture("container"))).unwrap();
        assert_eq!(items, []);
    }

    #[test]
    fn logind_counts_open_sessions_of_people_only() {
        let sessions = vec![
            session("alice", "user", "active", false),
            session("alice", "user", "online", true),
            session("root", "user-early", "active", true),
            session("bob", "user", "closing", true),
            session("gdm", "greeter", "online", false),
            session("alice", "manager", "active", false),
        ];
        // logind wins: utmp is ignored when logind answers.
        let utmp = record(7, "mallory", "192.0.2.9");
        let items = collect_with("logind", &[("run/utmp", &utmp)], Logind(Ok(sessions))).unwrap();
        assert_eq!(
            counts(&items),
            [("users", 2), ("sessions", 3), ("remote", 2)]
        );
    }

    #[test]
    fn no_session_is_zero_not_unknown() {
        let utmp = record(7, "mallory", "192.0.2.9");
        let items = collect_with("empty", &[("run/utmp", &utmp)], Logind(Ok(Vec::new()))).unwrap();
        assert_eq!(
            counts(&items),
            [("users", 0), ("sessions", 0), ("remote", 0)]
        );
    }

    #[test]
    fn failing_logind_fails_the_section_without_utmp() {
        let utmp = record(7, "mallory", "192.0.2.9");
        let err = collect_with(
            "bus-error",
            &[("run/utmp", &utmp)],
            Logind(Err(SystemdError::Call("GetAll: boom".into()))),
        )
        .unwrap_err();
        assert_eq!(err.reason(), "bus error");
        assert_eq!(err.to_string(), "systemd call failed: GetAll: boom");
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
