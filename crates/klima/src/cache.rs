//! Last known server responses, and the history of unit states.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use jiff::{SignedDuration, Timestamp};
use melcloud::{AtaView, DeviceId, FanSpeed, OperationMode, WeatherKind};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::files::{self, append_json_line, read_json, write_json};
use crate::paths::Paths;

/// How long `klima status` trusts a cached unit state.
pub const STATE_TTL: SignedDuration = SignedDuration::from_mins(5);
/// How long the cached device list is trusted. Devices and their capabilities rarely change.
pub const DEVICES_TTL: SignedDuration = SignedDuration::from_hours(24);
/// How long unit states stay in the history.
pub const HISTORY_KEPT: SignedDuration = SignedDuration::from_hours(24 * 90);

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

/// A unit state from the history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub at: Timestamp,
    pub power: bool,
    pub mode: OperationMode,
    pub target: f64,
    pub room: f64,
    pub outside: Option<f64>,
    /// `None` when the line names no known speed.
    pub fan: Option<FanSpeed>,
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
        #[serde(default)]
        outside: Option<f64>,
        #[serde(default)]
        fan: Option<String>,
    }
    let Ok(text) = std::fs::read_to_string(paths.history()) else {
        return Vec::new();
    };
    let mut samples: Vec<Sample> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Stored>(line).ok())
        .filter(|stored| stored.device == device && stored.at >= since)
        .map(|stored| Sample {
            at: stored.at,
            power: stored.power,
            mode: stored.mode.parse().unwrap_or(OperationMode::Other(0)),
            target: stored.target,
            room: stored.room,
            outside: stored.outside,
            fan: stored.fan.and_then(|fan| fan.parse().ok()),
        })
        .collect();
    // Processes appending at nearly the same moment can write out of order.
    samples.sort_by_key(|sample| sample.at);
    samples
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
    let _lock = files::lock(&paths.history_lock())?;
    append_json_line(&paths.history(), &record)?;
    prune_history(&paths.history(), now)
}

/// Drops states older than [`HISTORY_KEPT`] once the oldest is a day past it, so the file is
/// rewritten about once a day. Lines that cannot be read go too. Runs under the history lock.
fn prune_history(path: &Path, now: Timestamp) -> Result<(), AppError> {
    #[derive(Deserialize)]
    struct Stored {
        at: Timestamp,
    }
    let cutoff = now - HISTORY_KEPT;
    let read_at = |line: &str| serde_json::from_str::<Stored>(line).ok().map(|s| s.at);
    let mut first = String::new();
    File::open(path)
        .and_then(|file| BufReader::new(file).read_line(&mut first))
        .map_err(|e| AppError::io(path, e))?;
    let due = read_at(&first).is_none_or(|at| at < cutoff - SignedDuration::from_hours(24));
    if first.is_empty() || !due {
        return Ok(());
    }
    let text = std::fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;
    let kept: String = text
        .lines()
        .filter(|line| read_at(line).is_some_and(|at| at >= cutoff))
        .flat_map(|line| [line, "\n"])
        .collect();
    files::replace(path, |file: &mut File| -> io::Result<()> {
        file.write_all(kept.as_bytes())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_readable_samples_of_the_unit_since_a_time_in_order() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::under(home.path());
        std::fs::create_dir_all(paths.history().parent().unwrap()).unwrap();
        let lines = [
            r#"{"at":"2026-01-15T08:00:00Z","device":1000001,"power":true,"mode":"heat","target":22.0,"room":20.5}"#,
            r#"{"at":"2026-01-15T09:00:00Z","device":1000001,"power":false,"mode":"cool","target":24.0,"room":21.0,"fan":"auto"}"#,
            r#"{"at":"2026-01-15T09:30:00Z","device":1000002,"power":true,"mode":"heat","target":22.0,"room":19.0}"#,
            "not a sample",
            r#"{"at":"2026-01-15T10:00:00Z","device":1000001,"power":true,"mode":"mode 9","target":22.5,"room":21.5,"outside":-3.5,"fan":"speed 9"}"#,
            r#"{"at":"2026-01-15T09:15:00Z","device":1000001,"power":true,"mode":"heat","target":22.0,"room":20.0}"#,
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
                    outside: None,
                    fan: Some(FanSpeed::Auto),
                },
                Sample {
                    at: "2026-01-15T09:15:00Z".parse().unwrap(),
                    power: true,
                    mode: OperationMode::Heat,
                    target: 22.0,
                    room: 20.0,
                    outside: None,
                    fan: None,
                },
                Sample {
                    at: "2026-01-15T10:00:00Z".parse().unwrap(),
                    power: true,
                    mode: OperationMode::Other(0),
                    target: 22.5,
                    room: 21.5,
                    outside: Some(-3.5),
                    fan: None,
                },
            ]
        );
    }

    #[test]
    fn history_older_than_kept_goes_once_it_is_a_day_past() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("history.jsonl");
        let now: Timestamp = "2026-06-01T12:00:00Z".parse().unwrap();
        let line = |days_ago: i64| {
            let at = now - SignedDuration::from_hours(24 * days_ago);
            format!(r#"{{"at":"{at}","device":1}}"#)
        };
        let write = |lines: &[String]| std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let lines = || std::fs::read_to_string(&path).unwrap().lines().count();

        write(&[line(90), line(10), line(1)]);
        prune_history(&path, now).unwrap();
        assert_eq!(lines(), 3, "the oldest is not a day past 90 days yet");

        write(&[line(92), line(91), "not json".into(), line(10), line(1)]);
        prune_history(&path, now).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            [line(10), line(1)].join("\n") + "\n"
        );

        std::fs::write(&path, "").unwrap();
        prune_history(&path, now).unwrap();
        assert_eq!(lines(), 0);
    }
}
