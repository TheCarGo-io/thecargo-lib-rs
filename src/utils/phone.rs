use phonenumber::{Mode, country};

pub fn normalize_us_phone(value: &str) -> Option<String> {
    let stripped = value.trim();
    if stripped.is_empty() {
        return None;
    }
    let candidate = digits_unless_vanity(stripped);
    let number = phonenumber::parse(Some(country::US), &candidate).ok()?;
    phonenumber::is_valid(&number).then(|| number.format().mode(Mode::E164).to_string())
}

fn digits_unless_vanity(value: &str) -> String {
    if value.chars().filter(char::is_ascii_alphabetic).count() >= 3 {
        return value.to_owned();
    }
    let plus = if value.starts_with('+') { "+" } else { "" };
    format!("{plus}{}", phone_digits(value))
}

pub fn format_us_phone(value: &str) -> Option<String> {
    let normalized = normalize_us_phone(value)?;
    let number = phonenumber::parse(Some(country::US), &normalized).ok()?;
    Some(number.format().mode(Mode::National).to_string())
}

pub fn phone_digits(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn national_formats_normalize_to_e164() {
        assert_eq!(normalize_us_phone("(212) 555-0187").as_deref(), Some("+12125550187"));
        assert_eq!(normalize_us_phone("212.555.0187").as_deref(), Some("+12125550187"));
        assert_eq!(normalize_us_phone("+1 212 555 0187").as_deref(), Some("+12125550187"));
        assert_eq!(normalize_us_phone("12125550187").as_deref(), Some("+12125550187"));
    }

    #[test]
    fn invalid_or_blank_numbers_are_none() {
        assert_eq!(normalize_us_phone(""), None);
        assert_eq!(normalize_us_phone("   "), None);
        assert_eq!(normalize_us_phone("12"), None);
        assert_eq!(normalize_us_phone("123-456-7890"), None);
        assert_eq!(normalize_us_phone("not a phone"), None);
        assert_eq!(normalize_us_phone("773-555-01e1"), None);
    }

    #[test]
    fn a_vanity_number_still_spells_its_digits() {
        assert_eq!(normalize_us_phone("1-800-FLOWERS").as_deref(), Some("+18003569377"));
    }

    #[test]
    fn national_format_prints_the_us_shape() {
        assert_eq!(format_us_phone("+12125550187").as_deref(), Some("(212) 555-0187"));
        assert_eq!(format_us_phone("nope"), None);
    }
}
