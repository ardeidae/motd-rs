//! `/proc/loadavg`: `"1.56 2.67 3.05 9/1264 1949508"`. The first three fields
//! are the load averages over 1, 5 and 15 minutes, printed by the kernel with
//! exactly two decimals.

use super::ParseError;

const SOURCE: &str = "/proc/loadavg";

/// Returns the three load averages in hundredths (`1.56` → `156`): integers,
/// so the model stays exact and comparable.
///
/// # Errors
///
/// Fewer than three fields, or a field that is not a non-negative decimal
/// with at most two decimals.
pub fn parse(input: &str) -> Result<[u32; 3], ParseError> {
    let mut fields = input.split_ascii_whitespace();
    let mut averages = [0; 3];
    for average in &mut averages {
        let field = fields
            .next()
            .ok_or_else(|| ParseError::new(SOURCE, "fewer than 3 fields"))?;
        *average = hundredths(field)
            .ok_or_else(|| ParseError::new(SOURCE, format!("'{field}' is not a load average")))?;
    }
    Ok(averages)
}

/// `"1.56"` → 156, `"12"` → 1200, `"0.5"` → 50. Digits only: no sign, no
/// exponent, at most two decimals.
fn hundredths(field: &str) -> Option<u32> {
    let (whole, fraction) = field.split_once('.').unwrap_or((field, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || !digits(fraction) || fraction.len() > 2 {
        return None;
    }
    let fraction = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u32>().ok()? * 10,
        _ => fraction.parse::<u32>().ok()?,
    };
    whole
        .parse::<u32>()
        .ok()?
        .checked_mul(100)?
        .checked_add(fraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_input() {
        // rpi4-ubuntu and an idle machine.
        assert_eq!(
            parse("1.56 2.67 3.05 9/1264 1949508\n"),
            Ok([156, 267, 305])
        );
        assert_eq!(parse("0.00 0.01 0.05 1/131 4242\n"), Ok([0, 1, 5]));
        assert_eq!(parse("123.45 99.00 1.10 1/1 1"), Ok([12345, 9900, 110]));
    }

    #[test]
    fn lenient_on_decimals_count() {
        assert_eq!(parse("1 2.5 3.25"), Ok([100, 250, 325]));
    }

    #[test]
    fn malformed_input() {
        for bad in [
            "",
            "\n",
            "1.00 2.00",
            "1.00 x 3.00",
            "-1.00 2.00 3.00",
            "+1.00 2.00 3.00",
            "1.000 2.00 3.00",
            ".50 2.00 3.00",
            "1e3 2.00 3.00",
            "1,50 2.00 3.00",
            "99999999.00 1 1",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
