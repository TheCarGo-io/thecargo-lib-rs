use axum::http::StatusCode;
use chrono::{Datelike, NaiveDate, TimeDelta};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::handlers::ApiError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub enum Period {
    #[serde(rename = "today")]
    Today,
    #[default]
    #[serde(rename = "7d")]
    Last7d,
    #[serde(rename = "30d")]
    Last30d,
    #[serde(rename = "mtd")]
    Mtd,
    #[serde(rename = "qtd")]
    Qtd,
    #[serde(rename = "custom")]
    Custom,
}

impl std::str::FromStr for Period {
    type Err = UnknownPeriod;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        [Self::Today, Self::Last7d, Self::Last30d, Self::Mtd, Self::Qtd, Self::Custom]
            .into_iter()
            .find(|period| period.value() == raw)
            .ok_or(UnknownPeriod)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown period")]
pub struct UnknownPeriod;

impl Period {
    pub fn value(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Last7d => "7d",
            Self::Last30d => "30d",
            Self::Mtd => "mtd",
            Self::Qtd => "qtd",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DateWindow {
    pub date_from: NaiveDate,
    pub date_to: NaiveDate,
}

impl DateWindow {
    pub fn new(date_from: NaiveDate, date_to: NaiveDate) -> Self {
        Self { date_from, date_to }
    }

    pub fn span_days(&self) -> i64 {
        (self.date_to - self.date_from).num_days() + 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedPeriod {
    pub period: Period,
    pub current: DateWindow,
    pub prior: DateWindow,
    pub label: String,
}

fn short(day: NaiveDate) -> String {
    day.format("%b %-d").to_string()
}

fn format_label(period: Period, w: DateWindow) -> String {
    let date_part = if w.date_from == w.date_to {
        short(w.date_from)
    } else if w.date_from.month() == w.date_to.month() {
        format!("{}–{}", short(w.date_from), w.date_to.day())
    } else {
        format!("{}–{}", short(w.date_from), short(w.date_to))
    };
    match period {
        Period::Today => format!("Today · {date_part}"),
        Period::Last7d => format!("Last 7 days · {date_part}"),
        Period::Last30d => format!("Last 30 days · {date_part}"),
        Period::Mtd => format!("Month to date · {date_part}"),
        Period::Qtd => format!("Quarter to date · {date_part}"),
        Period::Custom => date_part,
    }
}

fn days(n: i64) -> TimeDelta {
    TimeDelta::days(n)
}

fn first_of_month(day: NaiveDate) -> NaiveDate {
    day.with_day(1).unwrap_or(day)
}

fn quarter_start(day: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(day.year(), (day.month() - 1) / 3 * 3 + 1, 1).unwrap_or(day)
}

// The window a preset covers on `today` in the organization's zone, and the window before it to
// compare with.
pub fn resolve_period(
    period: Period,
    today: NaiveDate,
    date_from: Option<NaiveDate>,
    date_to: Option<NaiveDate>,
) -> Result<ResolvedPeriod, ApiError> {
    let (current, prior) = match period {
        Period::Today => (DateWindow::new(today, today), DateWindow::new(today - days(1), today - days(1))),
        Period::Last7d => (DateWindow::new(today - days(6), today), DateWindow::new(today - days(13), today - days(7))),
        Period::Last30d => {
            (DateWindow::new(today - days(29), today), DateWindow::new(today - days(59), today - days(30)))
        }
        Period::Mtd => {
            let start = first_of_month(today);
            let prev_month_end = start - days(1);
            let prev_month_start = first_of_month(prev_month_end);
            let prior_end = if today.day() <= prev_month_end.day() {
                prev_month_end.with_day(today.day()).unwrap_or(prev_month_end).min(prev_month_end)
            } else {
                prev_month_end
            };
            (DateWindow::new(start, today), DateWindow::new(prev_month_start, prior_end))
        }
        Period::Qtd => {
            let start = quarter_start(today);
            let prev_q_end = start - days(1);
            let prev_q_start = quarter_start(prev_q_end);
            (
                DateWindow::new(start, today),
                DateWindow::new(prev_q_start, prev_q_end.min(prev_q_start + (today - start))),
            )
        }
        Period::Custom => {
            let (Some(date_from), Some(date_to)) = (date_from, date_to) else {
                return Err(ApiError::http(StatusCode::BAD_REQUEST, "period=custom requires date_from and date_to"));
            };
            if date_to < date_from {
                return Err(ApiError::http(StatusCode::BAD_REQUEST, "date_to must be on or after date_from"));
            }
            if (date_to - date_from).num_days() > 366 {
                return Err(ApiError::http(StatusCode::BAD_REQUEST, "Custom range cannot exceed 366 days"));
            }
            let current = DateWindow::new(date_from, date_to);
            let span = current.span_days();
            (current, DateWindow::new(date_from - days(span), date_from - days(1)))
        }
    };
    Ok(ResolvedPeriod { period, current, prior, label: format_label(period, current) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn month_to_date_compares_with_the_same_days_of_last_month() {
        let r = resolve_period(Period::Mtd, day(2026, 3, 31), None, None).unwrap();
        assert_eq!(r.current, DateWindow::new(day(2026, 3, 1), day(2026, 3, 31)));
        assert_eq!(r.prior, DateWindow::new(day(2026, 2, 1), day(2026, 2, 28)));
        assert_eq!(r.label, "Month to date · Mar 1–31");
    }

    #[test]
    fn quarter_to_date_and_its_label_across_months() {
        let r = resolve_period(Period::Qtd, day(2026, 10, 20), None, None).unwrap();
        assert_eq!(r.current.date_from, day(2026, 10, 1));
        assert_eq!(r.prior, DateWindow::new(day(2026, 7, 1), day(2026, 7, 20)));
        let week = resolve_period(Period::Last7d, day(2026, 10, 3), None, None).unwrap();
        assert_eq!(week.label, "Last 7 days · Sep 27–Oct 3");
    }

    #[test]
    fn a_custom_window_needs_both_ends_in_order() {
        assert!(resolve_period(Period::Custom, day(2026, 10, 3), Some(day(2026, 10, 1)), None).is_err());
        assert!(
            resolve_period(Period::Custom, day(2026, 10, 3), Some(day(2026, 10, 2)), Some(day(2026, 10, 1))).is_err()
        );
        let r =
            resolve_period(Period::Custom, day(2026, 10, 3), Some(day(2026, 8, 1)), Some(day(2026, 8, 31))).unwrap();
        assert_eq!(r.prior, DateWindow::new(day(2026, 7, 1), day(2026, 7, 31)));
        assert_eq!(r.label, "Aug 1–31");
    }
}
