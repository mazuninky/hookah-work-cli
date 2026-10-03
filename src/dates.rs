//! `--date`/`--from`/`--till` values and business-day resolution.
//!
//! The CRM counts a business day from the `midnight` hour of `GET /api/settings`, in the
//! CRM `timezone`. Booking lists accept `today`/`yesterday`/`tomorrow` and apply that shift
//! server-side; every other endpoint needs `YYYY-MM-DD`, so relative values are resolved here.

use std::cell::OnceCell;
use std::fmt;
use std::str::FromStr;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use serde_json::Value;
use tracing::{debug, warn};

use crate::client::Api;
use crate::error::{Error, Result};

/// A date argument: a day relative to the current business day, or a calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateArg {
    /// The current business day.
    Today,
    /// The business day before the current one.
    Yesterday,
    /// The business day after the current one.
    Tomorrow,
    /// A calendar date.
    On(Date),
}

impl DateArg {
    /// The value as the API spells it: `today`, `yesterday`, `tomorrow` or `YYYY-MM-DD`.
    #[must_use]
    pub fn literal(&self) -> String {
        self.to_string()
    }

    /// Day offset from the current business day; `None` for a calendar date.
    #[must_use]
    pub fn day_offset(&self) -> Option<i64> {
        match self {
            DateArg::Today => Some(0),
            DateArg::Yesterday => Some(-1),
            DateArg::Tomorrow => Some(1),
            DateArg::On(_) => None,
        }
    }
}

impl fmt::Display for DateArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateArg::Today => f.write_str("today"),
            DateArg::Yesterday => f.write_str("yesterday"),
            DateArg::Tomorrow => f.write_str("tomorrow"),
            DateArg::On(date) => write!(f, "{date}"),
        }
    }
}

impl FromStr for DateArg {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let value = value.trim();
        match value.to_ascii_lowercase().as_str() {
            "today" => return Ok(DateArg::Today),
            "yesterday" => return Ok(DateArg::Yesterday),
            "tomorrow" => return Ok(DateArg::Tomorrow),
            _ => {}
        }
        let is_iso_shape = value.len() == 10
            && value.char_indices().all(|(i, c)| {
                if i == 4 || i == 7 {
                    c == '-'
                } else {
                    c.is_ascii_digit()
                }
            });
        if !is_iso_shape {
            return Err(format!(
                "'{value}' is not a date; use today, yesterday, tomorrow or YYYY-MM-DD"
            ));
        }
        value
            .parse::<Date>()
            .map(DateArg::On)
            .map_err(|_| format!("'{value}' is not a valid calendar date (YYYY-MM-DD)"))
    }
}

/// clap value parser for date arguments.
pub fn parse_date_arg(value: &str) -> std::result::Result<DateArg, String> {
    value.parse()
}

/// The business date `offset_days` away from the one containing `now`: the date of
/// (`now` in `tz` − `midnight_hour` hours), plus the offset. Pure, so the shift is testable.
pub fn business_date(
    now: Timestamp,
    tz: &TimeZone,
    midnight_hour: i64,
    offset_days: i64,
) -> Result<Date> {
    let local = now.to_zoned(tz.clone()).datetime();
    local
        .checked_sub(midnight_hour.hours())
        .and_then(|shifted| shifted.date().checked_add(offset_days.days()))
        .map_err(|e| Error::InvalidInput(format!("date out of range: {e}")))
}

/// The CRM's business-day settings.
#[derive(Debug, Clone, PartialEq)]
pub struct BusinessDay {
    /// CRM time zone (the system zone when the CRM reports an unknown one).
    pub tz: TimeZone,
    /// Hour at which a new business day starts.
    pub midnight_hour: i64,
}

impl BusinessDay {
    /// Reads `timezone` and `midnight` from a `GET /api/settings` body. An unknown time zone
    /// or unparsable hour is logged and replaced by the system zone / hour 0.
    #[must_use]
    pub fn from_settings(settings: &Value) -> Self {
        let tz_name = settings.get("timezone").and_then(Value::as_str);
        let tz = match tz_name.map(TimeZone::get) {
            Some(Ok(tz)) => tz,
            Some(Err(err)) => {
                warn!(
                    "unknown CRM time zone {:?} ({err}); using the system time zone",
                    tz_name.unwrap_or_default()
                );
                TimeZone::system()
            }
            None => {
                warn!("CRM settings carry no time zone; using the system time zone");
                TimeZone::system()
            }
        };
        let midnight_hour = match settings.get("midnight") {
            Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
            Some(Value::Number(n)) => n.as_i64(),
            _ => None,
        }
        .filter(|hour| (0..24).contains(hour))
        .unwrap_or_else(|| {
            warn!("CRM settings carry no valid 'midnight' hour; assuming 0");
            0
        });
        Self { tz, midnight_hour }
    }
}

/// Resolves [`DateArg`]s to `YYYY-MM-DD`, fetching `/api/settings` at most once per invocation.
pub struct DateResolver<'a> {
    api: &'a dyn Api,
    now: Timestamp,
    day: OnceCell<BusinessDay>,
}

impl fmt::Debug for DateResolver<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DateResolver")
            .field("now", &self.now)
            .field("day", &self.day)
            .finish_non_exhaustive()
    }
}

impl<'a> DateResolver<'a> {
    /// A resolver that evaluates relative dates at `now`.
    #[must_use]
    pub fn new(api: &'a dyn Api, now: Timestamp) -> Self {
        Self {
            api,
            now,
            day: OnceCell::new(),
        }
    }

    /// `YYYY-MM-DD` for `arg`; relative values cost one settings request (cached).
    pub fn resolve(&self, arg: DateArg) -> Result<String> {
        let Some(offset) = arg.day_offset() else {
            return Ok(arg.literal());
        };
        let day = self.business_day()?;
        let date = business_date(self.now, &day.tz, day.midnight_hour, offset)?;
        debug!("{arg} resolved to {date}");
        Ok(date.to_string())
    }

    fn business_day(&self) -> Result<&BusinessDay> {
        if let Some(day) = self.day.get() {
            return Ok(day);
        }
        let settings = self.api.get("/api/settings", &[])?.into_json()?;
        Ok(self
            .day
            .get_or_init(|| BusinessDay::from_settings(&settings)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::FakeApi;
    use jiff::civil::date;
    use serde_json::json;

    fn yekaterinburg() -> TimeZone {
        TimeZone::get("Asia/Yekaterinburg").unwrap()
    }

    #[test]
    fn parses_keywords_and_dates() {
        assert_eq!("today".parse::<DateArg>().unwrap(), DateArg::Today);
        assert_eq!("Yesterday".parse::<DateArg>().unwrap(), DateArg::Yesterday);
        assert_eq!(
            "2026-09-30".parse::<DateArg>().unwrap(),
            DateArg::On(date(2026, 9, 30))
        );
        for bad in ["2026-13-01", "2026-9-30", "30.09.2026", "now", ""] {
            assert!(bad.parse::<DateArg>().is_err(), "{bad}");
        }
    }

    #[test]
    fn literal_round_trips() {
        assert_eq!(DateArg::Tomorrow.literal(), "tomorrow");
        assert_eq!(DateArg::On(date(2026, 8, 1)).literal(), "2026-08-01");
    }

    #[test]
    fn before_midnight_shift_belongs_to_previous_day() {
        // 03:00 local (UTC+5) on 2026-10-03 with a 06:00 business-day start.
        let now: Timestamp = "2026-10-02T22:00:00Z".parse().unwrap();
        let tz = yekaterinburg();
        assert_eq!(business_date(now, &tz, 6, 0).unwrap(), date(2026, 10, 2));
        assert_eq!(business_date(now, &tz, 6, -1).unwrap(), date(2026, 10, 1));
        assert_eq!(business_date(now, &tz, 6, 1).unwrap(), date(2026, 10, 3));
    }

    #[test]
    fn after_midnight_shift_is_same_day() {
        // 07:00 local on 2026-10-03.
        let now: Timestamp = "2026-10-03T02:00:00Z".parse().unwrap();
        assert_eq!(
            business_date(now, &yekaterinburg(), 6, 0).unwrap(),
            date(2026, 10, 3)
        );
    }

    #[test]
    fn the_shift_hour_itself_starts_the_new_business_day() {
        let tz = yekaterinburg();
        let at = |utc: &str| business_date(utc.parse().unwrap(), &tz, 6, 0).unwrap();
        // 05:59:59 and 06:00:00 local on 2026-10-03.
        assert_eq!(at("2026-10-03T00:59:59Z"), date(2026, 10, 2));
        assert_eq!(at("2026-10-03T01:00:00Z"), date(2026, 10, 3));
    }

    #[test]
    fn crm_time_zone_decides_the_date_not_utc() {
        // 01:30 on 2026-10-04 in Auckland (NZDT, UTC+13) while UTC is still on 10-03.
        let now: Timestamp = "2026-10-03T12:30:00Z".parse().unwrap();
        let auckland = TimeZone::get("Pacific/Auckland").unwrap();
        assert_eq!(
            business_date(now, &auckland, 0, 0).unwrap(),
            date(2026, 10, 4)
        );
        assert_eq!(
            business_date(now, &auckland, 6, 0).unwrap(),
            date(2026, 10, 3)
        );
        // 20:00 on 2026-10-02 in Los Angeles (PDT, UTC-7) while UTC is already on 10-03.
        let now: Timestamp = "2026-10-03T03:00:00Z".parse().unwrap();
        let los_angeles = TimeZone::get("America/Los_Angeles").unwrap();
        assert_eq!(
            business_date(now, &los_angeles, 0, 0).unwrap(),
            date(2026, 10, 2)
        );
    }

    #[test]
    fn shift_and_offsets_cross_the_year_boundary() {
        // 03:00 local on 2027-01-01.
        let now: Timestamp = "2026-12-31T22:00:00Z".parse().unwrap();
        let tz = yekaterinburg();
        assert_eq!(business_date(now, &tz, 6, 0).unwrap(), date(2026, 12, 31));
        assert_eq!(business_date(now, &tz, 6, -1).unwrap(), date(2026, 12, 30));
        assert_eq!(business_date(now, &tz, 6, 1).unwrap(), date(2027, 1, 1));
    }

    #[test]
    fn settings_accept_numeric_hours_and_reject_out_of_range_ones() {
        let numeric =
            BusinessDay::from_settings(&json!({"midnight": 5, "timezone": "Pacific/Auckland"}));
        assert_eq!(numeric.midnight_hour, 5);
        assert_eq!(numeric.tz, TimeZone::get("Pacific/Auckland").unwrap());
        let out_of_range = BusinessDay::from_settings(&json!({"midnight": "24"}));
        assert_eq!(out_of_range.midnight_hour, 0);
        assert_eq!(out_of_range.tz, TimeZone::system());
    }

    #[test]
    fn settings_parsing_tolerates_bad_values() {
        let day =
            BusinessDay::from_settings(&json!({"midnight": "6", "timezone": "Asia/Yekaterinburg"}));
        assert_eq!(day.midnight_hour, 6);
        assert_eq!(day.tz, yekaterinburg());
        let fallback =
            BusinessDay::from_settings(&json!({"midnight": "x", "timezone": "Mars/Base"}));
        assert_eq!(fallback.midnight_hour, 0);
        assert_eq!(fallback.tz, TimeZone::system());
    }

    #[test]
    fn resolver_fetches_settings_once_and_skips_absolute_dates() {
        let api = FakeApi::new().with_json(
            "/api/settings",
            json!({"midnight": "6", "timezone": "Asia/Yekaterinburg"}),
        );
        let now: Timestamp = "2026-10-02T22:00:00Z".parse().unwrap();
        let resolver = DateResolver::new(&api, now);
        assert_eq!(
            resolver.resolve(DateArg::On(date(2026, 1, 2))).unwrap(),
            "2026-01-02"
        );
        assert!(api.requests().is_empty());
        assert_eq!(resolver.resolve(DateArg::Today).unwrap(), "2026-10-02");
        assert_eq!(resolver.resolve(DateArg::Yesterday).unwrap(), "2026-10-01");
        assert_eq!(api.requests().len(), 1);
    }
}
