// Values written the way pydantic writes them, where serde's default differs.

// A UTC instant as pydantic prints one: no fraction when it is a whole second, else six digits.
pub mod utc_datetime {
    use chrono::{DateTime, Timelike, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn format(moment: &DateTime<Utc>) -> String {
        if moment.nanosecond() == 0 {
            moment.format("%Y-%m-%dT%H:%M:%SZ").to_string()
        } else {
            moment.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
        }
    }

    pub fn serialize<S: Serializer>(moment: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format(moment))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DateTime<Utc>, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        super::datetime_from_json(&raw).map_err(serde::de::Error::custom)
    }

    pub mod option {
        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(moment: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error> {
            match moment {
                Some(moment) => serializer.serialize_str(&super::format(moment)),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error> {
            let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
            match raw {
                None | Some(serde_json::Value::Null) => Ok(None),
                Some(value) => super::super::datetime_from_json(&value).map(Some).map_err(serde::de::Error::custom),
            }
        }
    }
}

// pydantic parses datetimes with speedate, falls back to a date-only parse, and reports that
// fallback's error text; the accepted shapes and the messages are mirrored here.
pub fn parse_pydantic_datetime(raw: &str) -> Result<chrono::DateTime<chrono::Utc>, String> {
    speedate::parse(raw)
        .map_err(|_first| format!("Input should be a valid datetime or date, {}", speedate::date_error(raw.as_bytes())))
}

// pydantic's lax mode also accepts unix timestamps: seconds, or milliseconds above 20 000 000 000.
pub fn datetime_from_timestamp(value: f64) -> Result<chrono::DateTime<chrono::Utc>, String> {
    let seconds = if value.abs() > 20_000_000_000.0 { value / 1000.0 } else { value };
    let whole = seconds.trunc();
    if !(-11_676_096_000.0..=253_402_300_799.0).contains(&whole) {
        return Err("Input should be a valid datetime, dates before 1600 or after 9999 are not supported".to_owned());
    }
    #[expect(clippy::cast_possible_truncation, reason = "guarded by the range check above")]
    let secs = whole as i64;
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "the fraction is within 0..1")]
    let nanos = ((seconds - whole).abs() * 1e9) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
        .ok_or_else(|| "Input should be a valid datetime".to_owned())
}

// pydantic's `date` accepts a plain date or a datetime at midnight; the error texts are mirrored.
pub fn parse_pydantic_date(raw: &str) -> Result<chrono::NaiveDate, String> {
    if let Ok(day) = speedate::parse_date(raw.as_bytes()) {
        return Ok(day);
    }
    let at = speedate::parse(raw).map_err(|_first| {
        format!("Input should be a valid date or datetime, {}", speedate::date_error(raw.as_bytes()))
    })?;
    if at.time() == chrono::NaiveTime::MIN {
        Ok(at.date_naive())
    } else {
        Err("Datetimes provided to dates should have zero time - e.g. `2020-01-01T00:00:00`".to_owned())
    }
}

pub fn datetime_from_json(value: &serde_json::Value) -> Result<chrono::DateTime<chrono::Utc>, String> {
    match value {
        serde_json::Value::String(text) => parse_pydantic_datetime(text),
        serde_json::Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| "Input should be a valid datetime".to_owned())
            .and_then(datetime_from_timestamp),
        _ => Err("Input should be a valid datetime".to_owned()),
    }
}

mod speedate {
    use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

    const TOO_SHORT: &str = "input is too short";
    const EXTRA: &str = "unexpected extra characters at the end of the input";

    fn digits(bytes: &[u8], start: usize, len: usize, invalid: &'static str) -> Result<u32, &'static str> {
        let slice = bytes.get(start..start + len).ok_or(TOO_SHORT)?;
        let mut value = 0_u32;
        for byte in slice {
            if !byte.is_ascii_digit() {
                return Err(invalid);
            }
            value = value * 10 + u32::from(byte - b'0');
        }
        Ok(value)
    }

    fn date(bytes: &[u8]) -> Result<NaiveDate, &'static str> {
        if bytes.len() < 10 {
            return Err(TOO_SHORT);
        }
        let year = digits(bytes, 0, 4, "invalid character in year")?;
        if bytes[4] != b'-' {
            return Err("invalid date separator, expected `-`");
        }
        let month = digits(bytes, 5, 2, "invalid character in month")?;
        if bytes[7] != b'-' {
            return Err("invalid date separator, expected `-`");
        }
        let day = digits(bytes, 8, 2, "invalid character in day")?;
        if !(1..=12).contains(&month) {
            return Err("month value is outside expected range of 1-12");
        }
        let Ok(year) = i32::try_from(year) else { return Err("invalid character in year") };
        NaiveDate::from_ymd_opt(year, month, day).ok_or("day value is outside expected range")
    }

    fn time(bytes: &[u8]) -> Result<(NaiveTime, usize), &'static str> {
        if bytes.len() < 5 {
            return Err(TOO_SHORT);
        }
        let hour = digits(bytes, 0, 2, "invalid character in hour")?;
        if bytes[2] != b':' {
            return Err("invalid time separator, expected `:`");
        }
        let minute = digits(bytes, 3, 2, "invalid character in minute")?;
        let mut position = 5;
        let mut second = 0;
        let mut micros = 0_u32;
        if bytes.get(position) == Some(&b':') {
            second = digits(bytes, position + 1, 2, "invalid character in second")?;
            position += 3;
            if bytes.get(position) == Some(&b'.') {
                position += 1;
                let start = position;
                while bytes.get(position).is_some_and(u8::is_ascii_digit) {
                    position += 1;
                }
                let fraction = &bytes[start..position];
                if fraction.is_empty() {
                    return Err("second fraction digits missing after `.`");
                }
                if fraction.len() > 6 {
                    return Err("second fraction value is more than 6 digits long");
                }
                for (index, byte) in fraction.iter().enumerate() {
                    micros += u32::from(byte - b'0') * 10_u32.pow(5 - u32::try_from(index).unwrap_or(5));
                }
            }
        }
        if hour > 23 {
            return Err("hour value is outside expected range of 0-23");
        }
        if minute > 59 {
            return Err("minute value is outside expected range of 0-59");
        }
        if second > 59 {
            return Err("second value is outside expected range of 0-59");
        }
        let time = NaiveTime::from_hms_micro_opt(hour, minute, second, micros).ok_or(TOO_SHORT)?;
        Ok((time, position))
    }

    fn offset(bytes: &[u8]) -> Result<Option<i32>, &'static str> {
        match bytes.first() {
            None => Ok(None),
            Some(b'Z' | b'z') => {
                if bytes.len() > 1 {
                    return Err(EXTRA);
                }
                Ok(Some(0))
            }
            Some(sign @ (b'+' | b'-')) => {
                let hours = digits(bytes, 1, 2, "invalid timezone hour")?;
                let (minutes, consumed) = match bytes.get(3) {
                    None => (0, 3),
                    Some(b':') => (digits(bytes, 4, 2, "invalid timezone minute")?, 6),
                    Some(byte) if byte.is_ascii_digit() => (digits(bytes, 3, 2, "invalid timezone minute")?, 5),
                    Some(_) => return Err(EXTRA),
                };
                if bytes.len() > consumed {
                    return Err(EXTRA);
                }
                let total = i32::try_from(hours * 3600 + minutes * 60).unwrap_or(i32::MAX);
                if total >= 24 * 3600 {
                    return Err("timezone offset must be less than 24 hours");
                }
                Ok(Some(if *sign == b'-' { -total } else { total }))
            }
            Some(_) => Err("invalid timezone sign"),
        }
    }

    pub fn parse_date(bytes: &[u8]) -> Result<NaiveDate, &'static str> {
        let day = date(bytes)?;
        if bytes.len() != 10 {
            return Err(EXTRA);
        }
        Ok(day)
    }

    pub fn date_error(bytes: &[u8]) -> &'static str {
        match date(bytes) {
            Err(error) => error,
            Ok(_) if bytes.len() > 10 => EXTRA,
            Ok(_) => TOO_SHORT,
        }
    }

    pub fn parse(raw: &str) -> Result<DateTime<Utc>, &'static str> {
        let bytes = raw.as_bytes();
        let day = date(bytes)?;
        if bytes.len() == 10 {
            return Ok(day.and_time(NaiveTime::MIN).and_utc());
        }
        if !matches!(bytes[10], b'T' | b't' | b' ' | b'_') {
            return Err("invalid datetime separator, expected `T`, `t`, `_` or space");
        }
        let (clock, consumed) = time(&bytes[11..])?;
        let naive = NaiveDateTime::new(day, clock);
        match offset(&bytes[11 + consumed..])? {
            None => Ok(naive.and_utc()),
            Some(seconds) => {
                let zone = FixedOffset::east_opt(seconds).ok_or("timezone offset must be less than 24 hours")?;
                zone.from_local_datetime(&naive).single().map(|at| at.with_timezone(&Utc)).ok_or(TOO_SHORT)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::utc_datetime::format;

    #[test]
    fn datetimes_parse_and_fail_like_pydantic() {
        use super::parse_pydantic_datetime as parse;
        assert_eq!(parse("2027-01-15T10:00:00Z").unwrap().to_rfc3339(), "2027-01-15T10:00:00+00:00");
        assert_eq!(parse("2027-01-15 10:00").unwrap().to_rfc3339(), "2027-01-15T10:00:00+00:00");
        assert_eq!(parse("2027-01-15").unwrap().to_rfc3339(), "2027-01-15T00:00:00+00:00");
        assert_eq!(parse("2027-01-15T10:00:00.250+02:00").unwrap().to_rfc3339(), "2027-01-15T08:00:00.250+00:00");
        assert_eq!(parse("soon").unwrap_err(), "Input should be a valid datetime or date, input is too short");
        assert_eq!(
            parse("2027-13-15T10:00:00Z").unwrap_err(),
            "Input should be a valid datetime or date, month value is outside expected range of 1-12"
        );
        assert_eq!(
            parse("2027-01-15X10:00:00Z").unwrap_err(),
            "Input should be a valid datetime or date, unexpected extra characters at the end of the input"
        );
        assert_eq!(parse("2027-1-15").unwrap_err(), "Input should be a valid datetime or date, input is too short");
        assert_eq!(
            parse("2027/01/15").unwrap_err(),
            "Input should be a valid datetime or date, invalid date separator, expected `-`"
        );
        assert_eq!(super::datetime_from_timestamp(12345.0).unwrap().to_rfc3339(), "1970-01-01T03:25:45+00:00");
        assert_eq!(
            super::datetime_from_timestamp(1_700_000_000_500.0).unwrap().to_rfc3339(),
            "2023-11-14T22:13:20.500+00:00"
        );
        assert_eq!(
            parse("2027-01-15T10:00:00Zjunk").unwrap_err(),
            "Input should be a valid datetime or date, unexpected extra characters at the end of the input"
        );
        assert_eq!(
            parse("2027-01-15T10:00:00+25:00").unwrap_err(),
            "Input should be a valid datetime or date, unexpected extra characters at the end of the input"
        );
    }

    #[test]
    fn fractions_print_as_pydantic_prints_them() {
        let whole = Utc.with_ymd_and_hms(2026, 10, 20, 15, 59, 12).unwrap();
        assert_eq!(format(&whole), "2026-10-20T15:59:12Z");
        assert_eq!(format(&(whole + chrono::TimeDelta::microseconds(400_000))), "2026-10-20T15:59:12.400000Z");
        assert_eq!(format(&(whole + chrono::TimeDelta::microseconds(120))), "2026-10-20T15:59:12.000120Z");
    }
}
