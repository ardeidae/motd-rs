//! A logind session file, `/run/systemd/sessions/<id>`: `KEY=value` lines.
//!
//! systemd marks these files "private data, do not parse": the stable API is
//! `sd-login` or D-Bus. They are read anyway because one small file per
//! session costs far less than one D-Bus call per session (listing sessions
//! over D-Bus does not return their class or remote flag), and the keys used
//! here (`USER`, `CLASS`, `STATE`, `REMOTE`) have kept their meaning from
//! systemd 219 to today. A missing key makes the session uncountable, not
//! the run fail.

use super::ParseError;

const SOURCE: &str = "/run/systemd/sessions/*";

/// The fields of a session that the users section needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// `USER`: login name.
    pub user: String,
    /// `CLASS`: `user` for a person's login; `greeter`, `lock-screen`,
    /// `background`, `manager` and others are not people.
    pub class: String,
    /// `STATE`: `online`, `active` or `closing` (logged out, processes left).
    pub state: String,
    /// `REMOTE=1`: opened over the network (ssh).
    pub remote: bool,
}

impl Session {
    /// A person who is logged in now.
    #[must_use]
    pub fn is_open_user_session(&self) -> bool {
        self.class == "user" && self.state != "closing"
    }
}

/// Parses one session file.
///
/// # Errors
///
/// No `USER` or no `CLASS` line.
pub fn parse(input: &str) -> Result<Session, ParseError> {
    let (mut user, mut class, mut state, mut remote) = (None, None, String::new(), false);
    for line in input.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "USER" => user = Some(value.to_owned()),
            "CLASS" => class = Some(value.to_owned()),
            "STATE" => value.clone_into(&mut state),
            "REMOTE" => remote = value == "1",
            _ => {}
        }
    }
    Ok(Session {
        user: user.ok_or_else(|| ParseError::new(SOURCE, "no USER line"))?,
        class: class.ok_or_else(|| ParseError::new(SOURCE, "no CLASS line"))?,
        state,
        remote,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_input() {
        // rpi4-ubuntu, an ssh login.
        let input = "\
# This is private data. Do not parse.
UID=1000
USER=user1
ACTIVE=1
IS_DISPLAY=1
STATE=active
REMOTE=1
TYPE=tty
CLASS=user
REMOTE_HOST=10.99.3.148
SERVICE=sshd
";
        let session = parse(input).unwrap();
        assert_eq!(
            session,
            Session {
                user: "user1".into(),
                class: "user".into(),
                state: "active".into(),
                remote: true,
            }
        );
        assert!(session.is_open_user_session());
    }

    #[test]
    fn not_people_or_not_open() {
        let s = |class: &str, state: &str| {
            parse(&format!("USER=gdm\nCLASS={class}\nSTATE={state}\n")).unwrap()
        };
        assert!(!s("greeter", "online").is_open_user_session());
        assert!(!s("manager", "active").is_open_user_session());
        assert!(!s("background", "active").is_open_user_session());
        assert!(!s("user", "closing").is_open_user_session());
        assert!(s("user", "online").is_open_user_session());
        // A local session.
        assert!(!parse("USER=a\nCLASS=user\n").unwrap().remote);
    }

    #[test]
    fn malformed_input() {
        for bad in [
            "",
            "garbage",
            "USER=a\n",
            "CLASS=user\n",
            "USER\nCLASS=user\n",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
