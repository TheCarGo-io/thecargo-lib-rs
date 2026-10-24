// Values written the way pydantic writes them, where serde's default differs.

// A UTC instant as pydantic prints one: no fraction when it is a whole second, else six digits.
pub mod utc_datetime {
    use chrono::{DateTime, Timelike, Utc};
    use serde::Serializer;

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

    pub mod option {
        use chrono::{DateTime, Utc};
        use serde::Serializer;

        pub fn serialize<S: Serializer>(moment: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error> {
            match moment {
                Some(moment) => serializer.serialize_str(&super::format(moment)),
                None => serializer.serialize_none(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::utc_datetime::format;

    #[test]
    fn fractions_print_as_pydantic_prints_them() {
        let whole = Utc.with_ymd_and_hms(2026, 10, 20, 15, 59, 12).unwrap();
        assert_eq!(format(&whole), "2026-10-20T15:59:12Z");
        assert_eq!(format(&(whole + chrono::TimeDelta::microseconds(400_000))), "2026-10-20T15:59:12.400000Z");
        assert_eq!(format(&(whole + chrono::TimeDelta::microseconds(120))), "2026-10-20T15:59:12.000120Z");
    }
}
