//! `/proc/<pid>/stat`: `"1946553 ((sd-pam)) S 1946549 …"`. The command name is
//! between parentheses and may itself contain spaces and parentheses, so the
//! fields that follow are found after the **last** `)`.

use super::ParseError;

const SOURCE: &str = "/proc/<pid>/stat";

/// `PF_KTHREAD` in the `flags` field (`include/linux/sched.h`): the task is a
/// kernel thread. Stable since Linux 2.6.27.
const PF_KTHREAD: u64 = 0x0020_0000;

/// The fields motd-rs uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PidStat {
    /// Field 3: `R`, `S`, `D`, `Z` (zombie), `I` (idle kernel thread)…
    pub state: char,
    /// Field 9 has `PF_KTHREAD`.
    pub kernel_thread: bool,
}

/// Parses one `stat` line.
///
/// # Errors
///
/// No `(…)` around the name, or missing or non-numeric fields after it.
pub fn parse(input: &str) -> Result<PidStat, ParseError> {
    let close = input
        .rfind(')')
        .filter(|&close| input.find('(').is_some_and(|open| open < close))
        .ok_or_else(|| ParseError::new(SOURCE, "no (name)"))?;
    let rest = input.get(close + 1..).unwrap_or_default();
    // state ppid pgrp session tty_nr tpgid flags
    let mut fields = rest.split_ascii_whitespace();
    let state_field = fields
        .next()
        .ok_or_else(|| ParseError::new(SOURCE, "no state"))?;
    let mut chars = state_field.chars();
    let (Some(state), None) = (chars.next(), chars.next()) else {
        return Err(ParseError::new(
            SOURCE,
            format!("bad state '{state_field}'"),
        ));
    };
    let flags = fields
        .nth(5)
        .and_then(|field| field.parse::<u64>().ok())
        .ok_or_else(|| ParseError::new(SOURCE, "no flags"))?;
    Ok(PidStat {
        state,
        kernel_thread: flags & PF_KTHREAD != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lines from the rpi4-ubuntu fixture, cut after the fields we read.
    const SYSTEMD: &str = "1 (systemd) S 0 1 1 0 -1 4194560 12998189";
    const KTHREADD: &str = "2 (kthreadd) S 0 0 0 0 -1 2129984 0";
    const KWORKER: &str = "106357 (kworker/R-iscsi) I 2 0 0 0 -1 69238880 0";
    const SD_PAM: &str = "1946553 ((sd-pam)) S 1946549 1946549 1946549 0 -1 4194624 75";

    #[test]
    fn real_input() {
        assert_eq!(
            parse(SYSTEMD),
            Ok(PidStat {
                state: 'S',
                kernel_thread: false
            })
        );
        assert_eq!(
            parse(KTHREADD),
            Ok(PidStat {
                state: 'S',
                kernel_thread: true
            })
        );
        assert_eq!(
            parse(KWORKER),
            Ok(PidStat {
                state: 'I',
                kernel_thread: true
            })
        );
        assert_eq!(
            parse(SD_PAM),
            Ok(PidStat {
                state: 'S',
                kernel_thread: false
            })
        );
    }

    #[test]
    fn name_with_spaces_and_parentheses() {
        let line = "42 (a) Z (b) c) Z 1 1 1 0 -1 0 0";
        assert_eq!(parse(line).map(|s| s.state), Ok('Z'));
    }

    #[test]
    fn malformed_input() {
        for bad in [
            "",
            "42",
            "42 (name",
            "42 name) S 1 1 1 0 -1 0",
            "42 )name( S 1 1 1 0 -1 0",
            "42 (name)",
            "42 (name) S 1 1 1 0 -1",
            "42 (name) SS 1 1 1 0 -1 0",
            "42 (name) S 1 1 1 0 -1 x",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
