//! Which date an object shows.

use super::modified_from_head;

const LAST_MODIFIED: &str = "Wed, 01 Oct 2025 10:00:00 GMT";
const LAST_MODIFIED_SECS: u64 = 1_759_312_800;

#[test]
fn the_sources_mtime_wins_over_the_upload_time() {
    // rclone's key and format, so a file Cmdr or rclone uploaded shows the date
    // it had on the disk it came from.
    assert_eq!(
        modified_from_head(Some("1354040105.123456789"), Some(LAST_MODIFIED)),
        Some(1_354_040_105)
    );
}

#[test]
fn without_an_mtime_the_upload_time_shows() {
    assert_eq!(modified_from_head(None, Some(LAST_MODIFIED)), Some(LAST_MODIFIED_SECS));
}

#[test]
fn an_mtime_that_doesnt_parse_falls_back_to_the_upload_time() {
    // Another tool's idea of the key, or garbage: never a made-up date.
    assert_eq!(
        modified_from_head(Some("yesterday"), Some(LAST_MODIFIED)),
        Some(LAST_MODIFIED_SECS)
    );
}

#[test]
fn no_date_at_all_is_no_date() {
    assert_eq!(modified_from_head(None, None), None);
}
