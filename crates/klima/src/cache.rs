//! Last known server responses, and the history of unit states.

use std::path::Path;

use jiff::{SignedDuration, Timestamp};
use melcloud::{AtaView, DeviceId, OperationMode, WeatherKind};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::files::{append_json_line, read_json, write_json};
use crate::paths::Paths;

/// How long `klima status` trusts a cached unit state.
pub const STATE_TTL: SignedDuration = SignedDuration::from_mins(5);
/// How long the cached device list is trusted. Devices and their capabilities rarely change.
pub const DEVICES_TTL: SignedDuration = SignedDuration::from_hours(24);

/// A server response and when it was fetched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cached<T> {
    pub fetched_at: Timestamp,
    pub value: T,
}

impl<T> Cached<T> {
    pub fn fresh(&self, ttl: SignedDuration, now: Timestamp) -> bool {
        self.fetched_at + ttl > now
    }
}

/// Reads a cached response. A cache file that cannot be read is treated as missing, since the
/// server can always send it again.
pub fn load<T: DeserializeOwned>(path: &Path) -> Option<Cached<T>> {
    read_json(path).ok().flatten()
}

pub fn store<T: Serialize>(path: &Path, value: &T, now: Timestamp) -> Result<(), AppError> {
    write_json(
        path,
        &Cached {
            fetched_at: now,
            value,
        },
    )
}

/// One line of `history.jsonl`: a unit state as read from the server.
#[derive(Serialize)]
struct Record<'a> {
    at: Timestamp,
    device: DeviceId,
    power: bool,
    mode: String,
    target: f64,
    room: f64,
    outside: Option<f64>,
    fan: String,
    error_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<&'a str>,
}

/// A unit state from the history, for charts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub at: Timestamp,
    pub power: bool,
    pub mode: OperationMode,
    pub target: f64,
    pub room: f64,
}

/// States of `device` read since `since`, oldest first. Lines that cannot be read are skipped.
pub fn history(paths: &Paths, device: DeviceId, since: Timestamp) -> Vec<Sample> {
    #[derive(Deserialize)]
    struct Stored {
        at: Timestamp,
        device: DeviceId,
        power: bool,
        mode: String,
        target: f64,
        room: f64,
    }
    let Ok(text) = std::fs::read_to_string(paths.history()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Stored>(line).ok())
        .filter(|stored| stored.device == device && stored.at >= since)
        .map(|stored| Sample {
            at: stored.at,
            power: stored.power,
            mode: stored.mode.parse().unwrap_or(OperationMode::Other(0)),
            target: stored.target,
            room: stored.room,
        })
        .collect()
}

/// Appends a state read from the server to the history used for charts.
pub fn record_history(paths: &Paths, view: &AtaView, now: Timestamp) -> Result<(), AppError> {
    let record = Record {
        at: now,
        device: view.device_id,
        power: view.power,
        mode: view.mode.to_string(),
        target: view.target_temperature,
        room: view.room_temperature,
        outside: view
            .weather
            .iter()
            .find(|w| w.kind == WeatherKind::Current)
            .map(|w| w.temperature),
        fan: view.fan_speed.to_string(),
        error_code: view.error_code,
        error_message: view.error_message.as_deref(),
    };
    append_json_line(&paths.history(), &record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_readable_samples_of_the_unit_since_a_time() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::under(home.path());
        std::fs::create_dir_all(paths.history().parent().unwrap()).unwrap();
        let lines = [
            r#"{"at":"2026-01-15T08:00:00Z","device":1000001,"power":true,"mode":"heat","target":22.0,"room":20.5}"#,
            r#"{"at":"2026-01-15T09:00:00Z","device":1000001,"power":false,"mode":"cool","target":24.0,"room":21.0,"fan":"auto"}"#,
            r#"{"at":"2026-01-15T09:30:00Z","device":1000002,"power":true,"mode":"heat","target":22.0,"room":19.0}"#,
            "not a sample",
            r#"{"at":"2026-01-15T10:00:00Z","device":1000001,"power":true,"mode":"mode 9","target":22.5,"room":21.5}"#,
        ];
        std::fs::write(paths.history(), lines.join("\n")).unwrap();
        let since: Timestamp = "2026-01-15T08:30:00Z".parse().unwrap();
        let samples = history(&paths, DeviceId(1_000_001), since);
        assert_eq!(
            samples,
            [
                Sample {
                    at: "2026-01-15T09:00:00Z".parse().unwrap(),
                    power: false,
                    mode: OperationMode::Cool,
                    target: 24.0,
                    room: 21.0,
                },
                Sample {
                    at: "2026-01-15T10:00:00Z".parse().unwrap(),
                    power: true,
                    mode: OperationMode::Other(0),
                    target: 22.5,
                    room: 21.5,
                },
            ]
        );
    }
}
