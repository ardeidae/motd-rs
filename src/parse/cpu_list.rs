//! Kernel CPU lists, as in `/sys/devices/system/cpu/online`: `"0-3"`,
//! `"0-3,6,8-9"`.

use super::ParseError;

const SOURCE: &str = "/sys/devices/system/cpu/online";

/// Returns the number of CPUs in the list.
///
/// # Errors
///
/// Empty list, a range that goes backwards, or anything but digits, `-`
/// and `,`.
pub fn count(input: &str) -> Result<u32, ParseError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(ParseError::new(SOURCE, "empty"));
    }
    let mut total: u32 = 0;
    for part in text.split(',') {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        let (first, last) = (number(first)?, number(last)?);
        let size = last
            .checked_sub(first)
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| ParseError::new(SOURCE, format!("bad range '{part}'")))?;
        total = total
            .checked_add(size)
            .ok_or_else(|| ParseError::new(SOURCE, "too many CPUs"))?;
    }
    Ok(total)
}

fn number(text: &str) -> Result<u32, ParseError> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::new(
            SOURCE,
            format!("'{text}' is not a CPU number"),
        ));
    }
    text.parse()
        .map_err(|_| ParseError::new(SOURCE, format!("'{text}' is out of range")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_input() {
        assert_eq!(count("0-3\n"), Ok(4)); // rpi4-ubuntu
        assert_eq!(count("0-19\n"), Ok(20)); // container (its host)
        assert_eq!(count("0\n"), Ok(1)); // rpi-zero
    }

    #[test]
    fn holes_and_singles() {
        // CPUs taken offline leave holes.
        assert_eq!(count("0-3,6,8-9"), Ok(7));
        assert_eq!(count("0,2,4"), Ok(3));
    }

    #[test]
    fn malformed_input() {
        for bad in [
            "",
            "\n",
            "3-1",
            "0-",
            "-3",
            "a",
            "0-3,",
            ",0",
            "0 3",
            "0--3",
            "99999999999",
        ] {
            assert!(count(bad).is_err(), "{bad:?}");
        }
        assert!(count("0-4294967295").is_err(), "count overflows u32");
    }
}
