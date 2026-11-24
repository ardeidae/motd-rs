//! systemd unit states, read over D-Bus with read-only methods only.
//!
//! Collectors go through the [`Systemd`] trait: [`DbusSystemd`] talks to the
//! real system bus, [`FakeSystemd`] answers from the fixture's `fake.toml`.

use std::fmt;

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

/// Read-only access to systemd's unit states.
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
}

/// The real systemd, over the system bus (`org.freedesktop.systemd1`).
///
/// Each query opens one connection and makes at most two calls. These methods
/// are read-only and need neither root nor polkit.
#[derive(Debug, Clone, Copy, Default)]
pub struct DbusSystemd;

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

const DESTINATION: &str = "org.freedesktop.systemd1";
const PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";

impl Systemd for DbusSystemd {
    fn query(&self, watched: &[String], with_failed: bool) -> Result<UnitsAnswer, SystemdError> {
        // zbus 6 is async only; its own small runtime runs the future on this
        // thread, which is the collector's dedicated thread.
        zbus::block_on(async {
            let conn = zbus::Connection::system()
                .await
                .map_err(|e| SystemdError::Unavailable(e.to_string()))?;
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
        .map_err(|e| match e {
            // systemd is not on the bus (non-systemd init with a D-Bus daemon).
            zbus::Error::FDO(ref fdo) if matches!(**fdo, zbus::fdo::Error::ServiceUnknown(_)) => {
                SystemdError::Unavailable(e.to_string())
            }
            _ => SystemdError::Call(format!("{method}: {e}")),
        })?;
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
/// `failed`. A fixture without a `[systemd]` table has no systemd at all
/// ([`NoSystemd`]).
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
}

/// A machine without systemd (containers, other init systems): every query
/// fails with [`SystemdError::Unavailable`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSystemd;

impl Systemd for NoSystemd {
    fn query(&self, _watched: &[String], _with_failed: bool) -> Result<UnitsAnswer, SystemdError> {
        Err(SystemdError::Unavailable(
            "no systemd on this machine".into(),
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
        let answer = DbusSystemd.query(&watched, true).unwrap();
        assert_eq!(answer.watched.len(), 2);
        assert_eq!(answer.watched[0].name, "dbus.service");
        assert_eq!(answer.watched[0].load_state, "loaded");
        assert_eq!(answer.watched[0].active_state, "active");
        assert_eq!(answer.watched[1].name, "motd-rs-does-not-exist.service");
        assert_eq!(answer.watched[1].load_state, "not-found");
        assert!(answer.failed.iter().all(|u| u.active_state == "failed"));
    }
}
