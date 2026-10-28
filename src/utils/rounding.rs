// Python's round(x, ndigits) for floats: the exact binary value of x is rounded half to even at
// `ndigits` decimals, and the result is the double nearest that decimal. Formatting through a
// string would round ties the other way, so this works on the exact value with integers.
pub fn py_round(x: f64, ndigits: u32) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let scale = 10_u128.pow(ndigits);
    let bits = x.abs().to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
    let fraction = u128::from(bits & ((1 << 52) - 1));
    let (mantissa, shift) = if exponent == 0 { (fraction, 1074) } else { (fraction | (1 << 52), 1075 - exponent) };

    // |x| * 10^ndigits = mantissa * scale / 2^shift, rounded half to even.
    let scaled = mantissa * scale;
    let rounded = if shift <= 0 {
        return x;
    } else if shift >= 127 {
        0
    } else {
        let quotient = scaled >> shift;
        let remainder = scaled & ((1_u128 << shift) - 1);
        let half = 1_u128 << (shift - 1);
        if remainder > half || (remainder == half && quotient & 1 == 1) { quotient + 1 } else { quotient }
    };
    #[expect(clippy::cast_precision_loss, reason = "the rounded value fits a double's integer range")]
    let magnitude = rounded as f64 / scale as f64;
    if x.is_sign_negative() { -magnitude } else { magnitude }
}

// Python's round(x) with no digits: half to even, and an int.
#[expect(clippy::cast_possible_truncation, reason = "a rounded float is whole; saturating at i64 bounds is the intent")]
pub fn py_round_int(x: f64) -> i64 {
    py_round(x, 0) as i64
}

#[cfg(test)]
mod tests {
    use super::{py_round, py_round_int};

    #[test]
    fn matches_python_round_on_ties_and_near_ties() {
        let cases: [(f64, f64); 12] = [
            (0.125, 0.12),
            (0.375, 0.38),
            (2.675, 2.67),
            (1234.125, 1234.12),
            (-0.125, -0.12),
            (1.005, 1.0),
            (98_765.435_000_1, 98_765.44),
            (0.0, 0.0),
            (1e-9, 0.0),
            (3881.535, 3881.53),
            (323.465, 323.46),
            (12_345_678.125, 12_345_678.12),
        ];
        for (x, want) in cases {
            assert_eq!(py_round(x, 2).to_bits(), want.to_bits(), "round({x}, 2)");
        }
    }

    #[test]
    fn whole_number_rounding_is_banker_rounding() {
        assert_eq!(py_round_int(2.5), 2);
        assert_eq!(py_round_int(12_345_678.5), 12_345_678);
        assert_eq!(py_round_int(-3.5), -4);
        assert_eq!(py_round(2.5, 0), 2.0);
        assert_eq!(py_round(3.5, 0), 4.0);
        assert_eq!(py_round(0.5, 0), 0.0);
    }
}

pub fn py_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0.0" } else { "0.0" }.to_owned();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let sign = if mantissa.starts_with('-') { "-" } else { "" };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if !(-4..16).contains(&exponent) {
        let (first, rest) = digits.split_at(1);
        let fraction = if rest.is_empty() { String::new() } else { format!(".{rest}") };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        return format!("{sign}{first}{fraction}e{exponent_sign}{:02}", exponent.abs());
    }
    let point = exponent + 1;
    if point <= 0 {
        let zeros = usize::try_from(-point).unwrap_or(0);
        return format!("{sign}0.{}{digits}", "0".repeat(zeros));
    }
    let point = usize::try_from(point).unwrap_or(0);
    if digits.len() <= point {
        return format!("{sign}{digits}{}.0", "0".repeat(point - digits.len()));
    }
    let (whole, fraction) = digits.split_at(point);
    format!("{sign}{whole}.{fraction}")
}

#[cfg(test)]
mod repr_tests {
    use super::py_float_repr;

    #[test]
    fn floats_print_like_python_str() {
        let cases = [
            (950.0, "950.0"),
            (1234.56, "1234.56"),
            (0.5, "0.5"),
            (0.05, "0.05"),
            (1e16, "1e+16"),
            (1.5e-7, "1.5e-07"),
            (100.0, "100.0"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (12_345_678_901_234.56, "12345678901234.56"),
            (0.0001, "0.0001"),
            (0.000_123_4, "0.0001234"),
            (123_456_789_012_345.67, "123456789012345.67"),
            (-42.5, "-42.5"),
        ];
        for (value, expected) in cases {
            assert_eq!(py_float_repr(value), expected, "{value}");
        }
    }
}
