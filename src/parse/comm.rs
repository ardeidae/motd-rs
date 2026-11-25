//! A process name, as in `/proc/<pid>/comm`: `"sshd\n"`.

use super::ParseError;

const SOURCE: &str = "/proc/<pid>/comm";

/// The kernel keeps at most 15 bytes of a name (`TASK_COMM_LEN` - 1).
const MAX_LEN: usize = 15;

/// Returns the name without its line break. The kernel prints any bytes the
/// program set (`prctl(PR_SET_NAME)`), spaces included, so only the shape is
/// checked.
///
/// # Errors
///
/// Empty, longer than the kernel allows, or holding more than one line: not
/// a `comm` file.
pub fn parse(input: &str) -> Result<&str, ParseError> {
    let name = input.strip_suffix('\n').unwrap_or(input);
    if name.is_empty() {
        return Err(ParseError::new(SOURCE, "empty name"));
    }
    if name.len() > MAX_LEN {
        return Err(ParseError::new(SOURCE, "name longer than 15 bytes"));
    }
    if name.contains('\n') {
        return Err(ParseError::new(SOURCE, "more than one line"));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_input() {
        // From the rpi4-ubuntu fixture.
        assert_eq!(parse("systemd\n"), Ok("systemd"));
        // Truncated by the kernel: `longhorn-manager`.
        assert_eq!(parse("longhorn-manage\n"), Ok("longhorn-manage"));
        assert_eq!(parse("kworker/R-iscsi\n"), Ok("kworker/R-iscsi"));
        assert_eq!(parse("Web Content\n"), Ok("Web Content"));
        // A read cut before the line break.
        assert_eq!(parse("sshd"), Ok("sshd"));
    }

    #[test]
    fn malformed_input() {
        for bad in ["", "\n", "a-much-longer-name\n", "two\nlines\n"] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
