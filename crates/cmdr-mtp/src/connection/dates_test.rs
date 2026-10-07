use std::time::{Duration, UNIX_EPOCH};

use super::*;

fn at(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> mtp_rs::DateTime {
    mtp_rs::DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        offset: None,
    }
}

// Regression: the listing used 30-day months and a rough leap-year count, so a
// phone's photo dated 2021-01-29 listed days off, and an upload's date couldn't
// round-trip.
#[test]
fn a_listed_date_is_the_exact_unix_second() {
    assert_eq!(convert_mtp_datetime(at(1970, 1, 1, 0, 0, 0)), Some(0));
    assert_eq!(convert_mtp_datetime(at(2021, 1, 29, 8, 30, 15)), Some(1_611_909_015));
    // Past February of a leap year, and the 2000 leap century.
    assert_eq!(convert_mtp_datetime(at(2024, 3, 1, 0, 0, 0)), Some(1_709_251_200));
    assert_eq!(convert_mtp_datetime(at(2000, 12, 31, 23, 59, 59)), Some(978_307_199));
}

#[test]
fn a_date_before_1970_lists_none_and_impossible_fields_clamp() {
    assert_eq!(convert_mtp_datetime(at(1969, 12, 31, 23, 59, 59)), None);
    // A device that sends month 0 or day 0 gets the first of it, not a panic.
    assert_eq!(
        convert_mtp_datetime(at(2021, 0, 0, 0, 0, 0)),
        convert_mtp_datetime(at(2021, 1, 1, 0, 0, 0))
    );
}

#[test]
fn an_upload_sends_the_calendar_fields_of_its_date_and_they_round_trip() {
    let date = UNIX_EPOCH + Duration::from_millis(1_611_909_015_750);
    let sent = mtp_datetime_from_system_time(date).expect("a 2021 date fits a PTP DateTime");
    assert_eq!(sent, at(2021, 1, 29, 8, 30, 15), "whole seconds; the fraction drops");
    assert_eq!(convert_mtp_datetime(sent), Some(1_611_909_015));

    let leap_day = UNIX_EPOCH + Duration::from_secs(1_709_164_800); // 2024-02-29
    let sent = mtp_datetime_from_system_time(leap_day).expect("fits");
    assert_eq!(sent, at(2024, 2, 29, 0, 0, 0));
}

#[test]
fn a_date_a_ptp_datetime_cant_spell_sends_none() {
    assert_eq!(mtp_datetime_from_system_time(UNIX_EPOCH - Duration::from_secs(1)), None);
    // PTP's year is four digits.
    assert_eq!(
        mtp_datetime_from_system_time(UNIX_EPOCH + Duration::from_secs(253_402_300_800)), // 10000-01-01
        None
    );
}
