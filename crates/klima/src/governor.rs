//! The request budget shared by every klima process.
//!
//! MELCloud throttles a whole account for hours after too many requests and publishes no limit,
//! so every request first asks the ledger. The ledger lives on disk and is updated under a file
//! lock, so a CLI command, the TUI and a status bar never add up past the budget.

use std::collections::BTreeMap;
use std::fmt;

use jiff::{SignedDuration, Timestamp};
use melcloud::DeviceId;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::files::{self, read_json, write_json};
use crate::output::{local_time, seconds_until};
use crate::paths::Paths;

/// Requests allowed in any hour, across all processes. Polling every 10 minutes needs 6.
pub const HOURLY_LIMIT: usize = 30;
/// Shortest time between two state reads of one unit.
pub const READ_SPACING: SignedDuration = SignedDuration::from_secs(60);
/// How long to stop after MELCloud answers HTTP 429.
pub const THROTTLE_PAUSE: SignedDuration = SignedDuration::from_hours(6);

const HOUR: SignedDuration = SignedDuration::from_hours(1);

/// What a request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Login,
    Devices,
    /// A read of the unit's state; reads of one unit are spaced by [`READ_SPACING`].
    State(DeviceId),
    /// A command to the unit.
    Command(DeviceId),
    /// The read that checks whether the unit applied a command. It is part of the command, so
    /// it is not held back by the spacing of ordinary reads.
    Confirm(DeviceId),
}

/// Why the ledger refused a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// MELCloud answered HTTP 429 earlier.
    Throttled { until: Timestamp },
    /// MELCloud locked logins after failed attempts.
    LoginLocked { until: Timestamp },
    /// [`HOURLY_LIMIT`] requests in the last hour.
    HourlyLimit { next_at: Timestamp },
    /// The unit was read less than [`READ_SPACING`] ago.
    TooSoon { next_at: Timestamp },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Throttled { until } => write!(
                f,
                "MELCloud throttled this account; requests are paused until {}",
                local_time(*until)
            ),
            Self::LoginLocked { until } => write!(
                f,
                "MELCloud locked logins after failed attempts; try again after {}",
                local_time(*until)
            ),
            Self::HourlyLimit { next_at } => write!(
                f,
                "the budget of {HOURLY_LIMIT} requests per hour is used up; next request possible at {}",
                local_time(*next_at)
            ),
            Self::TooSoon { next_at } => write!(
                f,
                "the unit was read moments ago; next read possible in {} s",
                seconds_until(*next_at)
            ),
        }
    }
}

/// Recent requests and active blocks, stored in `ledger.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ledger {
    requests: Vec<Timestamp>,
    last_reads: BTreeMap<DeviceId, Timestamp>,
    throttled_until: Option<Timestamp>,
    login_locked_until: Option<Timestamp>,
}

impl Ledger {
    /// Records `request` at `now` if the budget allows it.
    pub fn admit(&mut self, request: Request, now: Timestamp) -> Result<(), Refusal> {
        self.forget_before(now);
        if let Some(until) = self.throttled_until {
            return Err(Refusal::Throttled { until });
        }
        if let (Request::Login, Some(until)) = (request, self.login_locked_until) {
            return Err(Refusal::LoginLocked { until });
        }
        if self.requests.len() >= HOURLY_LIMIT {
            return Err(Refusal::HourlyLimit {
                next_at: self.requests[0] + HOUR,
            });
        }
        match request {
            Request::State(device) => {
                if let Some(&last) = self.last_reads.get(&device) {
                    return Err(Refusal::TooSoon {
                        next_at: last + READ_SPACING,
                    });
                }
                self.last_reads.insert(device, now);
            }
            Request::Confirm(device) => {
                self.last_reads.insert(device, now);
            }
            Request::Login | Request::Devices | Request::Command(_) => {}
        }
        self.requests.push(now);
        Ok(())
    }

    /// MELCloud answered HTTP 429: stop everything for [`THROTTLE_PAUSE`].
    pub fn throttled(&mut self, now: Timestamp) -> Timestamp {
        let until = now + THROTTLE_PAUSE;
        self.throttled_until = Some(until);
        until
    }

    /// MELCloud locked logins for `minutes`.
    pub fn login_locked(&mut self, now: Timestamp, minutes: u32) -> Timestamp {
        let until = now + SignedDuration::from_mins(i64::from(minutes.max(1)));
        self.login_locked_until = Some(until);
        until
    }

    /// Lifts a throttling pause; returns whether one was active.
    pub fn unblock(&mut self, now: Timestamp) -> bool {
        self.forget_before(now);
        self.throttled_until.take().is_some()
    }

    fn forget_before(&mut self, now: Timestamp) {
        self.requests.retain(|&at| at + HOUR > now);
        self.last_reads.retain(|_, &mut at| at + READ_SPACING > now);
        self.throttled_until = self.throttled_until.filter(|&until| until > now);
        self.login_locked_until = self.login_locked_until.filter(|&until| until > now);
    }
}

/// Requests in the last hour, for display. Reads without the lock; a writer replaces the file
/// atomically, so this sees the old or the new ledger.
pub fn requests_last_hour(paths: &Paths, now: Timestamp) -> usize {
    read_json::<Ledger>(&paths.ledger())
        .ok()
        .flatten()
        .map_or(0, |ledger| {
            ledger
                .requests
                .iter()
                .filter(|&&at| at + HOUR > now)
                .count()
        })
}

/// Runs `change` on the ledger while holding the lock that serializes all klima processes.
pub fn update<T>(paths: &Paths, change: impl FnOnce(&mut Ledger) -> T) -> Result<T, AppError> {
    let _lock = files::lock(&paths.ledger_lock())?;
    let mut ledger = read_json(&paths.ledger())?.unwrap_or_default();
    let result = change(&mut ledger);
    write_json(&paths.ledger(), &ledger)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: DeviceId = DeviceId(1);

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_second(1_800_000_000 + seconds).unwrap()
    }

    #[test]
    fn hourly_limit_counts_every_request_and_frees_up_after_an_hour() {
        let mut ledger = Ledger::default();
        for i in 0..HOURLY_LIMIT {
            let request = if i % 2 == 0 {
                Request::Devices
            } else {
                Request::Login
            };
            ledger
                .admit(request, at(i64::try_from(i).unwrap()))
                .unwrap();
        }
        assert_eq!(
            ledger.admit(Request::Devices, at(100)),
            Err(Refusal::HourlyLimit { next_at: at(3600) })
        );
        assert_eq!(ledger.admit(Request::Devices, at(3600)), Ok(()));
    }

    #[test]
    fn state_reads_of_one_unit_are_spaced() {
        let mut ledger = Ledger::default();
        ledger.admit(Request::State(UNIT), at(0)).unwrap();
        assert_eq!(
            ledger.admit(Request::State(UNIT), at(59)),
            Err(Refusal::TooSoon { next_at: at(60) })
        );
        assert_eq!(ledger.admit(Request::State(DeviceId(2)), at(59)), Ok(()));
        assert_eq!(ledger.admit(Request::State(UNIT), at(60)), Ok(()));
    }

    #[test]
    fn commands_and_their_checks_count_but_are_not_spaced() {
        let mut ledger = Ledger::default();
        ledger.admit(Request::State(UNIT), at(0)).unwrap();
        assert_eq!(ledger.admit(Request::Command(UNIT), at(1)), Ok(()));
        assert_eq!(ledger.admit(Request::Confirm(UNIT), at(4)), Ok(()));
        assert_eq!(ledger.admit(Request::Confirm(UNIT), at(5)), Ok(()));
        assert_eq!(ledger.requests.len(), 4);
        // A check counts as a read for the spacing of the next ordinary read.
        assert_eq!(
            ledger.admit(Request::State(UNIT), at(61)),
            Err(Refusal::TooSoon { next_at: at(65) })
        );
    }

    #[test]
    fn commands_respect_the_hourly_limit() {
        let mut ledger = Ledger::default();
        for i in 0..HOURLY_LIMIT {
            ledger
                .admit(Request::Devices, at(i64::try_from(i).unwrap()))
                .unwrap();
        }
        assert!(matches!(
            ledger.admit(Request::Command(UNIT), at(100)),
            Err(Refusal::HourlyLimit { .. })
        ));
    }

    #[test]
    fn refused_requests_are_not_counted() {
        let mut ledger = Ledger::default();
        ledger.admit(Request::State(UNIT), at(0)).unwrap();
        for second in 1..40 {
            let _ = ledger.admit(Request::State(UNIT), at(second));
        }
        assert_eq!(ledger.requests.len(), 1);
    }

    #[test]
    fn throttling_stops_everything_until_it_expires_or_is_lifted() {
        let mut ledger = Ledger::default();
        let until = ledger.throttled(at(0));
        assert_eq!(until, at(6 * 3600));
        assert_eq!(
            ledger.admit(Request::Devices, at(10)),
            Err(Refusal::Throttled { until })
        );
        assert_eq!(
            ledger.admit(Request::Login, at(10)),
            Err(Refusal::Throttled { until })
        );
        assert_eq!(ledger.admit(Request::Devices, until), Ok(()));

        ledger.throttled(at(7 * 3600));
        assert!(ledger.unblock(at(7 * 3600 + 1)));
        assert!(!ledger.unblock(at(7 * 3600 + 2)));
        assert_eq!(ledger.admit(Request::Devices, at(7 * 3600 + 3)), Ok(()));
    }

    #[test]
    fn login_lock_only_stops_logins() {
        let mut ledger = Ledger::default();
        let until = ledger.login_locked(at(0), 15);
        assert_eq!(until, at(15 * 60));
        assert_eq!(
            ledger.admit(Request::Login, at(60)),
            Err(Refusal::LoginLocked { until })
        );
        assert_eq!(ledger.admit(Request::Devices, at(60)), Ok(()));
        assert_eq!(ledger.admit(Request::Login, until), Ok(()));
    }

    #[test]
    fn ledger_survives_a_round_trip_through_disk() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::under(home.path());
        update(&paths, |ledger| {
            ledger.admit(Request::State(UNIT), Timestamp::now())
        })
        .unwrap()
        .unwrap();
        let refused = update(&paths, |ledger| {
            ledger.admit(Request::State(UNIT), Timestamp::now())
        })
        .unwrap();
        assert!(matches!(refused, Err(Refusal::TooSoon { .. })));
    }
}
