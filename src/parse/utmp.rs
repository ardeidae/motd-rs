//! `/run/utmp`: who is logged in, on systems without logind (`OpenRC`,
//! runit, some containers). A binary array of glibc `struct utmp` records.
//!
//! The layout is the same on every Linux architecture glibc supports, 32-bit
//! included (glibc keeps 32-bit times in it for that reason): 384 bytes per
//! record, fields in the machine's byte order. musl does not write utmp; on
//! such systems the file is absent or written by another libc.

use super::ParseError;

const SOURCE: &str = "/run/utmp";

/// `sizeof(struct utmp)` with glibc on Linux.
pub const RECORD_LEN: usize = 384;
/// `ut_type` value of a logged-in user.
const USER_PROCESS: i16 = 7;
/// `ut_type`: `short` at offset 0.
const TYPE_AT: usize = 0;
/// `ut_user`: `char[32]` at offset 44.
const USER_AT: usize = 44;
const USER_LEN: usize = 32;
/// `ut_host`: `char[256]` at offset 76.
const HOST_AT: usize = 76;
const HOST_LEN: usize = 256;

/// One logged-in user record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    /// `ut_user`.
    pub user: String,
    /// `ut_host` is set and is not an X display (`:0`): a network login.
    pub remote: bool,
}

/// The `USER_PROCESS` records of the file, in order. Other records (boot
/// time, run level, getty waiting for a login, dead processes) are skipped.
///
/// # Errors
///
/// The length is not a whole number of records: not a glibc utmp file, or
/// one caught mid-write.
pub fn parse(input: &[u8]) -> Result<Vec<Login>, ParseError> {
    if !input.len().is_multiple_of(RECORD_LEN) {
        return Err(ParseError::new(
            SOURCE,
            format!("{} bytes is not a multiple of {RECORD_LEN}", input.len()),
        ));
    }
    Ok(input
        .as_chunks::<RECORD_LEN>()
        .0
        .iter()
        .map(<[u8; RECORD_LEN]>::as_slice)
        .filter(|record| ut_type(record) == Some(USER_PROCESS))
        .filter_map(|record| {
            let user = c_string(record.get(USER_AT..USER_AT + USER_LEN)?);
            let host = c_string(record.get(HOST_AT..HOST_AT + HOST_LEN)?);
            (!user.is_empty()).then(|| Login {
                user,
                remote: !host.is_empty() && !host.starts_with(':'),
            })
        })
        .collect())
}

fn ut_type(record: &[u8]) -> Option<i16> {
    let bytes = record.get(TYPE_AT..TYPE_AT + 2)?;
    Some(i16::from_ne_bytes(bytes.try_into().ok()?))
}

/// A fixed-size field, NUL-padded (not always NUL-terminated when full).
fn c_string(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(field.get(..end).unwrap_or_default()).into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A utmp record, as glibc writes it on this machine.
    pub(crate) fn record(kind: i16, user: &str, host: &str) -> Vec<u8> {
        let mut r = vec![0_u8; RECORD_LEN];
        r[TYPE_AT..TYPE_AT + 2].copy_from_slice(&kind.to_ne_bytes());
        r[USER_AT..USER_AT + user.len()].copy_from_slice(user.as_bytes());
        r[HOST_AT..HOST_AT + host.len()].copy_from_slice(host.as_bytes());
        r
    }

    #[test]
    fn users_only() {
        let boot_time = 2;
        let login_process = 6; // getty waiting
        let dead_process = 8;
        let file = [
            record(boot_time, "reboot", ""),
            record(login_process, "LOGIN", ""),
            record(USER_PROCESS, "alice", ""),
            record(USER_PROCESS, "bob", "192.0.2.7"),
            record(USER_PROCESS, "carol", ":0"),
            record(dead_process, "dave", "192.0.2.8"),
        ]
        .concat();
        assert_eq!(
            parse(&file),
            Ok(vec![
                Login {
                    user: "alice".into(),
                    remote: false
                },
                Login {
                    user: "bob".into(),
                    remote: true
                },
                Login {
                    user: "carol".into(),
                    remote: false
                },
            ])
        );
    }

    #[test]
    fn full_width_fields_have_no_nul() {
        let user = "u".repeat(USER_LEN);
        let file = record(USER_PROCESS, &user, &"h".repeat(HOST_LEN));
        let [login] = parse(&file).unwrap().try_into().unwrap();
        assert_eq!(login.user, user, "stops at the field end, not in ut_host");
        assert!(login.remote);
    }

    #[test]
    fn empty_file_and_empty_user() {
        assert_eq!(parse(&[]), Ok(vec![]));
        assert_eq!(parse(&record(USER_PROCESS, "", "")), Ok(vec![]));
    }

    #[test]
    fn truncated_input() {
        let file = [
            record(USER_PROCESS, "alice", ""),
            record(USER_PROCESS, "bob", ""),
        ]
        .concat();
        for len in [1, RECORD_LEN - 1, RECORD_LEN + 1, file.len() - 1] {
            assert!(parse(&file[..len]).is_err(), "{len}");
        }
    }
}
