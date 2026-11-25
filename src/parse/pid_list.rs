//! A list of process ids separated by spaces, as in
//! `/proc/<pid>/task/<tid>/children`: `"3 4 5 6 "`.

use super::ParseError;

const SOURCE: &str = "/proc/<pid>/task/<tid>/children";

/// Returns how many ids the list holds. An empty list is valid (no children).
///
/// # Errors
///
/// An entry that is not a decimal number.
pub fn count(input: &str) -> Result<u64, ParseError> {
    let mut total: u64 = 0;
    for field in input.split_ascii_whitespace() {
        if !field.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseError::new(SOURCE, format!("'{field}' is not a pid")));
        }
        total += 1;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_input() {
        // Children of kthreadd on the dev machine: one line, trailing space.
        assert_eq!(count("3 4 5 6 7 9 12 13 \n"), Ok(8));
        assert_eq!(count(""), Ok(0));
        assert_eq!(count("\n"), Ok(0));
    }

    #[test]
    fn malformed_input() {
        for bad in ["3 x 5", "-3", "3,4", "3\u{0}"] {
            assert!(count(bad).is_err(), "{bad:?}");
        }
    }
}
