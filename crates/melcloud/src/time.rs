//! Timestamps. The API sends ISO 8601 without a time zone; state and session times are UTC.

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;

/// Reads a zone-less API time that is known to be UTC.
pub(crate) fn utc(time: DateTime) -> Option<Timestamp> {
    time.to_zoned(TimeZone::UTC)
        .ok()
        .map(|zoned| zoned.timestamp())
}
