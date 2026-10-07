//! MTP's zone-less calendar dates ↔ Unix seconds, both ways.
//!
//! A PTP `DateTime` carries calendar fields and no zone. Both directions read
//! the fields as UTC, so what a listing shows and what an upload sends are exact
//! inverses. The day math is Howard Hinnant's `days_from_civil` /
//! `civil_from_days` (proleptic Gregorian, exact for every year).

use std::time::{SystemTime, UNIX_EPOCH};

const SECS_PER_DAY: i64 = 86_400;

/// A listed `DateTime` as Unix seconds. `None` before 1970, which the
/// `FileEntry` vocabulary can't hold. A device's out-of-range month or day
/// clamps instead of failing the listing.
pub(super) fn convert_mtp_datetime(dt: mtp_rs::DateTime) -> Option<u64> {
    let days = days_from_civil(
        i64::from(dt.year),
        i64::from(dt.month.clamp(1, 12)),
        i64::from(dt.day.max(1)),
    );
    let secs = days * SECS_PER_DAY + i64::from(dt.hour) * 3600 + i64::from(dt.minute) * 60 + i64::from(dt.second);
    u64::try_from(secs).ok()
}

/// The `DateModified` an upload sends for a file last changed at `date`, in
/// whole seconds. `None` for a date PTP can't spell (before 1970, or past year
/// 9999), so the device stamps its own.
pub(crate) fn mtp_datetime_from_system_time(date: SystemTime) -> Option<mtp_rs::DateTime> {
    let secs = i64::try_from(date.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    let (year, month, day) = civil_from_days(secs.div_euclid(SECS_PER_DAY));
    if year > 9999 {
        return None;
    }
    let of_day = secs.rem_euclid(SECS_PER_DAY);
    Some(mtp_rs::DateTime {
        year: u16::try_from(year).ok()?,
        month: u8::try_from(month).ok()?,
        day: u8::try_from(day).ok()?,
        hour: u8::try_from(of_day / 3600).ok()?,
        minute: u8::try_from(of_day % 3600 / 60).ok()?,
        second: u8::try_from(of_day % 60).ok()?,
        offset: None,
    })
}

/// Days since 1970-01-01 for a calendar date (`month` 1–12).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The calendar date `days` after 1970-01-01, as `(year, month 1–12, day)`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
#[path = "dates_test.rs"]
mod dates_test;
