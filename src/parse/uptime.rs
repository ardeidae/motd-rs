//! `/proc/uptime`: `"<uptime seconds> <idle seconds>"`, both with decimals.

use super::ParseError;

const SOURCE: &str = "/proc/uptime";

/// Returns the uptime in whole seconds.
///
/// # Errors
///
/// Empty input, or a first field that is not a finite non-negative number.
pub fn parse(input: &str) -> Result<u64, ParseError> {
    let first = input
        .split_ascii_whitespace()
        .next()
        .ok_or_else(|| ParseError::new(SOURCE, "empty"))?;
    // Integer part only: "1642000.53" → 1642000. Avoids float → int casts.
    let whole = first.split_once('.').map_or(first, |(whole, _)| whole);
    whole
        .parse::<u64>()
        .map_err(|err| ParseError::new(SOURCE, format!("'{first}': {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_input() {
        assert_eq!(parse("1642000.53 6400000.10\n"), Ok(1_642_000));
        assert_eq!(parse("0.01 0.00\n"), Ok(0));
        assert_eq!(parse("42 1"), Ok(42));
    }

    #[test]
    fn malformed_input() {
        assert!(parse("").is_err());
        assert!(parse("   \n").is_err());
        assert!(parse("abc 1.0").is_err());
        assert!(parse("-5.0 1.0").is_err());
        assert!(parse(".5 1.0").is_err());
        assert!(parse("99999999999999999999999.0 1").is_err());
    }
}
