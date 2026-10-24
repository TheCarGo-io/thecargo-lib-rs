use chrono::{DateTime, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TimezoneError {
    #[error("not an IANA timezone name: {0:?}")]
    NotIana(String),
    #[error("unknown IANA timezone: {0:?}")]
    Unknown(String),
}

pub fn utc_now() -> DateTime<Utc> {
    Utc::now()
}

// The source of "now" for anything a request computes from it; tests pin it to the moment the
// fixtures end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    System,
    Fixed(DateTime<Utc>),
}

impl Clock {
    pub fn now(self) -> DateTime<Utc> {
        match self {
            Self::System => utc_now(),
            Self::Fixed(moment) => moment,
        }
    }

    pub fn today(self, tz: Tz) -> NaiveDate {
        self.now().with_timezone(&tz).date_naive()
    }
}

pub fn validate_iana(name: &str) -> Result<Tz, TimezoneError> {
    if name != "UTC" && !name.contains('/') {
        return Err(TimezoneError::NotIana(name.to_owned()));
    }
    name.parse::<Tz>().map_err(|_unknown| TimezoneError::Unknown(name.to_owned()))
}

pub fn business_today(tz: Tz) -> NaiveDate {
    utc_now().with_timezone(&tz).date_naive()
}

pub fn start_of_day_utc(day: NaiveDate, tz: Tz) -> DateTime<Utc> {
    local_to_utc(day.and_time(NaiveTime::MIN), tz)
}

pub fn end_of_day_utc(day: NaiveDate, tz: Tz) -> DateTime<Utc> {
    start_of_day_utc(day + TimeDelta::days(1), tz)
}

pub fn latest_instant_of_day(day: NaiveDate, tz: Tz) -> DateTime<Utc> {
    end_of_day_utc(day, tz) - TimeDelta::microseconds(1)
}

pub fn day_bounds_utc(day: NaiveDate, tz: Tz) -> (DateTime<Utc>, DateTime<Utc>) {
    (start_of_day_utc(day, tz), end_of_day_utc(day, tz))
}

pub fn range_bounds_utc(
    date_from: Option<NaiveDate>,
    date_to: Option<NaiveDate>,
    tz: Tz,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    (date_from.map(|day| start_of_day_utc(day, tz)), date_to.map(|day| end_of_day_utc(day, tz)))
}

// Python's zoneinfo reads a wall time with fold=0: an ambiguous time takes the earlier offset, and a
// time inside a spring-forward gap takes the offset in force before the gap.
fn local_to_utc(local: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(moment) | LocalResult::Ambiguous(moment, _) => moment.with_timezone(&Utc),
        LocalResult::None => {
            let before = tz.offset_from_utc_datetime(&(local - TimeDelta::hours(6))).fix();
            Utc.from_utc_datetime(&(local - before))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn utc(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    #[test]
    fn only_iana_names_are_accepted() {
        assert_eq!(validate_iana("America/New_York"), Ok(Tz::America__New_York));
        assert_eq!(validate_iana("UTC"), Ok(Tz::UTC));
        assert!(matches!(validate_iana("EST"), Err(TimezoneError::NotIana(_))));
        assert!(matches!(validate_iana("Mars/Olympus"), Err(TimezoneError::Unknown(_))));
    }

    #[test]
    fn a_new_york_day_is_cut_at_local_midnight() {
        let (start, end) = day_bounds_utc(day(2026, 10, 23), Tz::America__New_York);
        assert_eq!(start, utc("2026-10-23T04:00:00Z"));
        assert_eq!(end, utc("2026-10-24T04:00:00Z"));
    }

    #[test]
    fn the_spring_forward_day_is_23_hours_and_the_fall_back_day_25() {
        let (start, end) = day_bounds_utc(day(2026, 3, 8), Tz::America__New_York);
        assert_eq!((end - start).num_hours(), 23);
        let (start, end) = day_bounds_utc(day(2026, 11, 1), Tz::America__New_York);
        assert_eq!((end - start).num_hours(), 25);
    }

    #[test]
    fn a_midnight_inside_a_gap_uses_the_offset_before_it() {
        // Havana springs forward at midnight: 2026-03-08 00:00 does not exist there. Python gives 05:00 UTC.
        assert_eq!(start_of_day_utc(day(2026, 3, 8), Tz::America__Havana), utc("2026-03-08T05:00:00Z"));
    }

    #[test]
    fn the_inclusive_bound_is_one_microsecond_before_the_next_midnight() {
        assert_eq!(latest_instant_of_day(day(2026, 10, 23), Tz::UTC), utc("2026-10-23T23:59:59.999999Z"));
    }

    #[test]
    fn open_ranges_stay_open() {
        let (lower, upper) = range_bounds_utc(None, Some(day(2026, 1, 1)), Tz::UTC);
        assert_eq!(lower, None);
        assert_eq!(upper, Some(utc("2026-01-02T00:00:00Z")));
    }
}
