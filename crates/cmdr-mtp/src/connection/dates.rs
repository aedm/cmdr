//! MTP dates ↔ Unix seconds, both ways, over mtp-rs's `DateTime` calendar math.
//!
//! An upload sends UTC with a `Z` (Android reads it as UTC). A read honors a
//! date's own offset, and a zoneless one (what Android sends: the phone's local
//! wall clock) reads at [`ZONELESS_DATES_READ_AT`].

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mtp_rs::{DateTime, UtcOffset};

/// The offset a zoneless device date is read at. UTC means a phone in Stockholm
/// lists a photo one or two hours off its local clock, but listing, copying off,
/// and copying back all agree. Whether to read it at the Mac's own zone instead
/// is an open product decision: <https://github.com/vdavid/cmdr/issues/373> item 4.
const ZONELESS_DATES_READ_AT: UtcOffset = UtcOffset::UTC;

/// A device `DateTime` as Unix seconds. `None` before 1970, which the
/// `FileEntry` vocabulary can't hold, and for fields that don't form a real
/// date (mtp-rs already drops most of those at parse time).
pub(super) fn convert_mtp_datetime(dt: DateTime) -> Option<u64> {
    u64::try_from(dt.to_unix_seconds_with_fallback(ZONELESS_DATES_READ_AT)?).ok()
}

/// A device `DateTime` as the instant a read stream reports.
pub(crate) fn system_time_from_mtp_datetime(dt: DateTime) -> Option<SystemTime> {
    convert_mtp_datetime(dt).map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
}

/// The `DateModified` an upload sends for a file last changed at `date`, in
/// whole seconds of UTC. `None` for a date PTP can't spell (before 1970, or past
/// year 9999), so the device stamps its own.
pub(crate) fn mtp_datetime_from_system_time(date: SystemTime) -> Option<DateTime> {
    let secs = i64::try_from(date.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    DateTime::from_unix_seconds(secs, UtcOffset::UTC)
}

#[cfg(test)]
#[path = "dates_test.rs"]
mod dates_test;
