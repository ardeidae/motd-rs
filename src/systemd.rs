//! systemd unit states and logind sessions, read over D-Bus with read-only
//! methods only.
//!
//! Collectors go through the [`Systemd`] trait: [`DbusSystemd`] talks to the
//! real system bus, [`FakeBus`] answers from the fixture's `fake.toml`.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::Poll;

use serde::Deserialize;

/// State of one unit, as systemd reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitStatus {
    /// Full unit name (`sshd.service`).
    pub name: String,
    /// `Description=` of the unit, empty if unknown.
    #[serde(default)]
    pub description: String,
    /// `loaded`, `not-found`, `masked`, `error`, …
    pub load_state: String,
    /// `active`, `inactive`, `failed`, `activating`, …
    pub active_state: String,
    /// Type-specific state: `running`, `exited`, `dead`, …
    pub sub_state: String,
}

/// What one [`Systemd::query`] returns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnitsAnswer {
    /// The requested units, in request order. Units unknown to systemd are
    /// included with `load_state = "not-found"`.
    pub watched: Vec<UnitStatus>,
    /// Every unit in the `failed` state, watched or not. Empty when not asked.
    pub failed: Vec<UnitStatus>,
}

/// One logind session, as `org.freedesktop.login1` reports it.
///
/// In `fake.toml` a session is one string, `"<user> <class> <state>
/// remote|local"`: decoding a string costs far less code than a table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Session {
    /// Login name.
    pub user: String,
    /// `user` for a person's login (`user-early`, `user-incomplete` and
    /// `user-light` too, since systemd 256); `greeter`, `lock-screen`,
    /// `background`, `manager` and others are not people.
    pub class: String,
    /// `online`, `active` or `closing` (logged out, processes left).
    pub state: String,
    /// Opened over the network (ssh).
    pub remote: bool,
}

impl Session {
    /// A person who is logged in now. systemd 256 gives root's logins the
    /// class `user-early`, so the `user-*` variants count as well.
    #[must_use]
    pub fn is_open_user_session(&self) -> bool {
        let person = self.class == "user" || self.class.starts_with("user-");
        person && self.state != "closing"
    }
}

impl TryFrom<String> for Session {
    type Error = String;

    fn try_from(line: String) -> Result<Self, Self::Error> {
        let bad = || format!("session {line:?}: expected \"<user> <class> <state> remote|local\"");
        let mut words = line.split_whitespace();
        let (Some(user), Some(class), Some(state), Some(place), None) = (
            words.next(),
            words.next(),
            words.next(),
            words.next(),
            words.next(),
        ) else {
            return Err(bad());
        };
        let remote = match place {
            "remote" => true,
            "local" => false,
            _ => return Err(bad()),
        };
        Ok(Self {
            user: user.to_owned(),
            class: class.to_owned(),
            state: state.to_owned(),
            remote,
        })
    }
}

/// systemd could not be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemdError {
    /// No system bus, or systemd is not on it (containers, chroots, non-systemd
    /// init). Carries the reason.
    Unavailable(String),
    /// A method call failed or its answer had an unexpected shape.
    Call(String),
}

impl fmt::Display for SystemdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "systemd unavailable: {reason}"),
            Self::Call(reason) => write!(f, "systemd call failed: {reason}"),
        }
    }
}

impl std::error::Error for SystemdError {}

/// Read-only access to systemd's unit states and logind's sessions.
pub trait Systemd: Send + Sync + fmt::Debug {
    /// The state of each unit in `watched` (full names, `.service` included)
    /// and, if `with_failed`, the list of every failed unit.
    ///
    /// Blocks until the answer arrives: call it from a collector thread, which
    /// the runner abandons at the deadline.
    ///
    /// # Errors
    ///
    /// [`SystemdError::Unavailable`] when there is no system bus or no systemd
    /// on it, [`SystemdError::Call`] when a call fails.
    fn query(&self, watched: &[String], with_failed: bool) -> Result<UnitsAnswer, SystemdError>;

    /// Every session logind knows, open or closing, in no set order.
    ///
    /// Blocks like [`Systemd::query`].
    ///
    /// # Errors
    ///
    /// [`SystemdError::Unavailable`] when there is no system bus or no logind
    /// on it, [`SystemdError::Call`] when listing the sessions fails.
    fn sessions(&self) -> Result<Vec<Session>, SystemdError>;
}

/// The real systemd and logind, over the system bus
/// (`org.freedesktop.systemd1`, `org.freedesktop.login1`).
///
/// One connection, opened by the first call and shared by the collector
/// threads: opening one (authentication and `Hello`) costs about as much as
/// all the calls made on it. These methods are read-only and need neither
/// root nor polkit.
#[derive(Debug, Default)]
pub struct DbusSystemd {
    connection: OnceLock<Result<zbus::Connection, SystemdError>>,
}

impl DbusSystemd {
    /// The shared connection. A second thread asking while the first one
    /// connects waits for that attempt instead of making its own.
    fn connection(&self) -> Result<zbus::Connection, SystemdError> {
        self.connection
            .get_or_init(|| {
                zbus::block_on(zbus::Connection::system())
                    .map_err(|e| SystemdError::Unavailable(e.to_string()))
            })
            .clone()
    }
}

/// `ListUnits*` returns `a(ssssssouso)`: name, description, load state,
/// active state, sub state, followed unit, unit path, job id, job type, job path.
type RawUnit = (
    String,
    String,
    String,
    String,
    String,
    String,
    zbus::OwnedObjectPath,
    u32,
    String,
    zbus::OwnedObjectPath,
);

/// `ListSessions` returns `a(susso)`: id, uid, user name, seat, object path.
type RawSession = (String, u32, String, String, zbus::OwnedObjectPath);

const DESTINATION: &str = "org.freedesktop.systemd1";
const PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";

const LOGIN1: &str = "org.freedesktop.login1";
const LOGIN1_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER: &str = "org.freedesktop.login1.Manager";
const LOGIN1_SESSION: &str = "org.freedesktop.login1.Session";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// Session property calls in flight at once. The system bus caps the
/// replies a connection may await (128 by default for dbus-daemon); past
/// it the extra calls fail instead of waiting.
const SESSIONS_IN_FLIGHT: usize = 32;

impl Systemd for DbusSystemd {
    fn query(&self, watched: &[String], with_failed: bool) -> Result<UnitsAnswer, SystemdError> {
        let conn = self.connection()?;
        // zbus 6 is async only; its own small runtime runs the future on this
        // thread, which is the collector's dedicated thread.
        zbus::block_on(async {
            let mut answer = UnitsAnswer::default();
            if !watched.is_empty() {
                // systemd >= 230. One round trip for every watched unit.
                answer.watched = list_units(&conn, "ListUnitsByNames", watched).await?;
                // The answer order is not documented: restore the request order.
                answer.watched.sort_by_key(|unit| {
                    watched
                        .iter()
                        .position(|name| *name == unit.name)
                        .unwrap_or(usize::MAX)
                });
            }
            if with_failed {
                // systemd >= 227.
                answer.failed = list_units(&conn, "ListUnitsFiltered", &["failed"]).await?;
            }
            Ok(answer)
        })
    }

    fn sessions(&self) -> Result<Vec<Session>, SystemdError> {
        let conn = self.connection()?;
        zbus::block_on(async {
            let reply = conn
                .call_method(
                    Some(LOGIN1),
                    LOGIN1_PATH,
                    Some(LOGIN1_MANAGER),
                    "ListSessions",
                    &(),
                )
                .await
                .map_err(|e| bus_error("ListSessions", &e))?;
            let listed: Vec<RawSession> = reply
                .body()
                .deserialize()
                .map_err(|e| SystemdError::Call(format!("ListSessions: {e}")))?;
            // The listing has no class, state or remote flag: ask each
            // session, with the calls in flight together rather than one
            // round trip after the other.
            let mut sessions = Vec::with_capacity(listed.len());
            for chunk in listed.chunks(SESSIONS_IN_FLIGHT) {
                let calls = chunk
                    .iter()
                    .map(|(_, _, user, _, path)| session(&conn, user, path))
                    .collect();
                for result in join_all(calls).await {
                    if let Some(session) = result? {
                        sessions.push(session);
                    }
                }
            }
            Ok(sessions)
        })
    }
}

/// Maps a failed call: a name nobody owns on the bus (systemd or logind
/// not running, other init systems) is [`SystemdError::Unavailable`].
fn bus_error(method: &str, e: &zbus::Error) -> SystemdError {
    match e {
        zbus::Error::FDO(fdo) if matches!(**fdo, zbus::fdo::Error::ServiceUnknown(_)) => {
            SystemdError::Unavailable(e.to_string())
        }
        _ => SystemdError::Call(format!("{method}: {e}")),
    }
}

/// One session's properties. `None` when it ended since the listing (its
/// object is gone): a logout during the run is not an error.
async fn session(
    conn: &zbus::Connection,
    user: &str,
    path: &zbus::OwnedObjectPath,
) -> Result<Option<Session>, SystemdError> {
    let reply = match conn
        .call_method(
            Some(LOGIN1),
            path,
            Some(PROPERTIES),
            "GetAll",
            &(LOGIN1_SESSION,),
        )
        .await
    {
        Ok(reply) => reply,
        Err(zbus::Error::FDO(fdo)) if matches!(*fdo, zbus::fdo::Error::UnknownObject(_)) => {
            return Ok(None);
        }
        Err(e) => return Err(bus_error("GetAll", &e)),
    };
    let props: SessionProperties = reply
        .body()
        .deserialize()
        .map_err(|e| SystemdError::Call(format!("GetAll: {e}")))?;
    let missing = |name: &str| SystemdError::Call(format!("GetAll: no {name} property"));
    Ok(Some(Session {
        user: user.to_owned(),
        class: props.class.ok_or_else(|| missing("Class"))?,
        state: props.state.ok_or_else(|| missing("State"))?,
        remote: props.remote.ok_or_else(|| missing("Remote"))?,
    }))
}

/// The session properties used, decoded straight from the `a{sv}` answer;
/// the others are skipped. Typed fields keep the binary smaller than a map
/// of generic variants, which must handle every D-Bus type.
#[derive(Debug, zbus::DeserializeDict, zbus::Type)]
#[zbus(signature = "a{sv}")]
struct SessionProperties {
    #[zbus(rename = "Class")]
    class: Option<String>,
    #[zbus(rename = "State")]
    state: Option<String>,
    #[zbus(rename = "Remote")]
    remote: Option<bool>,
}

/// Runs `futures` together and returns their outputs in the same order:
/// each D-Bus call is sent on its first poll, so all are in flight before
/// any reply is awaited.
async fn join_all<F: Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut futures: Vec<Pin<Box<F>>> = futures.into_iter().map(Box::pin).collect();
    let mut outputs: Vec<Option<F::Output>> = futures.iter().map(|_| None).collect();
    std::future::poll_fn(move |cx| {
        let mut pending = false;
        for (future, output) in futures.iter_mut().zip(outputs.iter_mut()) {
            if output.is_none() {
                match future.as_mut().poll(cx) {
                    Poll::Ready(value) => *output = Some(value),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(outputs.drain(..).flatten().collect())
        }
    })
    .await
}

/// Calls one of the `ListUnits*` methods, whose single argument is a string
/// array (`as`), and converts the answer.
async fn list_units<S: AsRef<str> + serde::Serialize + zbus::Type>(
    conn: &zbus::Connection,
    method: &'static str,
    arg: &[S],
) -> Result<Vec<UnitStatus>, SystemdError> {
    let reply = conn
        .call_method(Some(DESTINATION), PATH, Some(MANAGER), method, &(arg,))
        .await
        .map_err(|e| bus_error(method, &e))?;
    let raw: Vec<RawUnit> = reply
        .body()
        .deserialize()
        .map_err(|e| SystemdError::Call(format!("{method}: {e}")))?;
    Ok(raw
        .into_iter()
        .map(
            |(name, description, load_state, active_state, sub_state, ..)| UnitStatus {
                name,
                description,
                load_state,
                active_state,
                sub_state,
            },
        )
        .collect())
}

/// A fake systemd for tests and `--sysroot`: the `[systemd]` table of
/// `fake.toml`. Behaves like systemd: a watched unit that is not listed comes
/// back as `not-found`, and `failed` lists the units whose `active_state` is
/// `failed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FakeSystemd {
    /// Every unit known to this fake systemd.
    pub units: Vec<UnitStatus>,
}

impl Systemd for FakeSystemd {
    fn query(&self, watched: &[String], with_failed: bool) -> Result<UnitsAnswer, SystemdError> {
        let watched = watched
            .iter()
            .map(|name| {
                self.units
                    .iter()
                    .find(|unit| unit.name == *name)
                    .cloned()
                    .unwrap_or_else(|| UnitStatus {
                        name: name.clone(),
                        description: String::new(),
                        load_state: "not-found".into(),
                        active_state: "inactive".into(),
                        sub_state: "dead".into(),
                    })
            })
            .collect();
        let failed = if with_failed {
            self.units
                .iter()
                .filter(|unit| unit.active_state == "failed")
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        Ok(UnitsAnswer { watched, failed })
    }

    /// systemd alone, without logind.
    fn sessions(&self) -> Result<Vec<Session>, SystemdError> {
        NoSystemd.sessions()
    }
}

/// A fake logind for tests and `--sysroot`: the `[logind]` table of
/// `fake.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FakeLogind {
    /// Every session, as `ListSessions` and the session properties give them.
    pub sessions: Vec<Session>,
}

/// The fake system bus of a fixture: systemd and logind each answer only
/// when their table is in `fake.toml` (a container has neither).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeBus {
    /// `[systemd]`, or no systemd.
    pub systemd: Option<FakeSystemd>,
    /// `[logind]`, or no logind.
    pub logind: Option<FakeLogind>,
}

impl Systemd for FakeBus {
    fn query(&self, watched: &[String], with_failed: bool) -> Result<UnitsAnswer, SystemdError> {
        match &self.systemd {
            Some(systemd) => systemd.query(watched, with_failed),
            None => NoSystemd.query(watched, with_failed),
        }
    }

    fn sessions(&self) -> Result<Vec<Session>, SystemdError> {
        match &self.logind {
            Some(logind) => Ok(logind.sessions.clone()),
            None => NoSystemd.sessions(),
        }
    }
}

/// A machine without systemd (containers, other init systems): every call
/// fails with [`SystemdError::Unavailable`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSystemd;

impl Systemd for NoSystemd {
    fn query(&self, _watched: &[String], _with_failed: bool) -> Result<UnitsAnswer, SystemdError> {
        Err(SystemdError::Unavailable(
            "no systemd on this machine".into(),
        ))
    }

    fn sessions(&self) -> Result<Vec<Session>, SystemdError> {
        Err(SystemdError::Unavailable(
            "no logind on this machine".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str, active: &str, sub: &str) -> UnitStatus {
        UnitStatus {
            name: name.into(),
            description: String::new(),
            load_state: "loaded".into(),
            active_state: active.into(),
            sub_state: sub.into(),
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn fake_answers_like_systemd() {
        let fake = FakeSystemd {
            units: vec![
                unit("sshd.service", "active", "running"),
                unit("backup.service", "failed", "failed"),
                unit("cron.service", "active", "running"),
            ],
        };
        let answer = fake
            .query(&names(&["nope.service", "sshd.service"]), true)
            .unwrap();
        assert_eq!(answer.watched.len(), 2);
        assert_eq!(answer.watched[0].name, "nope.service");
        assert_eq!(answer.watched[0].load_state, "not-found");
        assert_eq!(answer.watched[1], unit("sshd.service", "active", "running"));
        assert_eq!(
            answer.failed,
            vec![unit("backup.service", "failed", "failed")]
        );

        let answer = fake.query(&[], false).unwrap();
        assert_eq!(answer, UnitsAnswer::default());
    }

    #[test]
    fn no_systemd_is_unavailable() {
        let err = NoSystemd
            .query(&names(&["sshd.service"]), true)
            .unwrap_err();
        assert!(matches!(err, SystemdError::Unavailable(_)));
        assert!(err.to_string().starts_with("systemd unavailable: "));
    }

    /// Needs a real systemd on the system bus (run in CI with `--include-ignored`).
    #[test]
    #[ignore = "needs systemd on the system bus"]
    fn real_systemd() {
        let watched = names(&["dbus.service", "motd-rs-does-not-exist.service"]);
        let answer = DbusSystemd::default().query(&watched, true).unwrap();
        assert_eq!(answer.watched.len(), 2);
        assert_eq!(answer.watched[0].name, "dbus.service");
        assert_eq!(answer.watched[0].load_state, "loaded");
        assert_eq!(answer.watched[0].active_state, "active");
        assert_eq!(answer.watched[1].name, "motd-rs-does-not-exist.service");
        assert_eq!(answer.watched[1].load_state, "not-found");
        assert!(answer.failed.iter().all(|u| u.active_state == "failed"));
    }

    fn session(user: &str, class: &str, state: &str) -> Session {
        Session {
            user: user.into(),
            class: class.into(),
            state: state.into(),
            remote: false,
        }
    }

    #[test]
    fn open_user_sessions() {
        assert!(session("a", "user", "active").is_open_user_session());
        assert!(session("a", "user", "online").is_open_user_session());
        assert!(session("root", "user-early", "active").is_open_user_session());
        assert!(session("a", "user-incomplete", "online").is_open_user_session());
        assert!(!session("a", "user", "closing").is_open_user_session());
        for class in [
            "greeter",
            "lock-screen",
            "background",
            "manager",
            "users",
            "",
        ] {
            assert!(
                !session("gdm", class, "online").is_open_user_session(),
                "{class}"
            );
        }
    }

    #[test]
    fn fake_bus_answers_only_what_it_has() {
        let logind = FakeLogind {
            sessions: vec![session("a", "user", "active")],
        };
        let bus = FakeBus {
            systemd: None,
            logind: Some(logind.clone()),
        };
        assert_eq!(bus.sessions().unwrap(), logind.sessions);
        assert!(matches!(
            bus.query(&[], false),
            Err(SystemdError::Unavailable(_))
        ));
        let bus = FakeBus {
            systemd: Some(FakeSystemd::default()),
            logind: None,
        };
        assert!(bus.query(&[], false).is_ok());
        assert!(matches!(bus.sessions(), Err(SystemdError::Unavailable(_))));
    }

    #[test]
    fn join_all_keeps_the_order() {
        let outputs = zbus::block_on(join_all((0..5).map(|n| async move { n * 2 }).collect()));
        assert_eq!(outputs, [0, 2, 4, 6, 8]);
    }

    /// Needs logind on the system bus (run in CI with `--include-ignored`).
    /// Both calls share one connection, as the collectors do.
    #[test]
    #[ignore = "needs logind on the system bus"]
    fn real_logind() {
        let bus = DbusSystemd::default();
        let sessions = bus.sessions().unwrap();
        assert!(
            sessions
                .iter()
                .all(|s| !s.user.is_empty() && !s.class.is_empty())
        );
        assert!(bus.query(&names(&["dbus.service"]), false).is_ok());
    }
}
