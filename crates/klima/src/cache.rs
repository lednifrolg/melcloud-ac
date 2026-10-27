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
    let Ok(bytes) = std::fs::read(paths.history()) else {
        return Vec::new();
    };
    // Line by line as bytes, so one damaged line costs only itself.
    let mut samples: Vec<Sample> = bytes
        .split(|&byte| byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Stored>(line).ok())
        .filter(|stored| stored.device == device && stored.at >= since)
        .map(|stored| Sample {
            at: stored.at,
            power: stored.power,
            mode: stored_mode(&stored.mode),
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

/// A mode as the history stores it: its name, or `mode N` for a code klima has no name for.
fn stored_mode(text: &str) -> OperationMode {
    text.parse().unwrap_or_else(|_| {
        let code = text
            .strip_prefix("mode ")
            .and_then(|code| code.parse().ok());
        OperationMode::Other(code.unwrap_or(0))
    })
}

/// Adds a state read from the server at `now` to the history used for charts, after dropping
/// what is older than [`HISTORY_KEPT`] before `reference`: a time that does not depend on the
/// local clock being right, where there is one.
pub fn record_history(
    paths: &Paths,
    view: &AtaView,
    now: Timestamp,
    reference: Timestamp,
) -> Result<(), AppError> {
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
    let pruned = prune_history(&paths.history(), reference);
    append_json_line(&paths.history(), &record)?;
    pruned
}

/// Drops states older than [`HISTORY_KEPT`] before `now` once the oldest is a day past it, so the
/// file is rewritten about once a day. Lines that cannot be read go too. Runs under the history
/// lock.
fn prune_history(path: &Path, now: Timestamp) -> Result<(), AppError> {
    #[derive(Deserialize)]
    struct Stored {
        at: Timestamp,
    }
    let read_at = |line: &[u8]| serde_json::from_slice::<Stored>(line).ok().map(|s| s.at);
    let mut first = Vec::new();
    match File::open(path) {
        Ok(file) => BufReader::new(file)
            .read_until(b'\n', &mut first)
            .map_err(|e| AppError::io(path, e))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::io(path, e)),
    };
    let day = SignedDuration::from_hours(24);
    let first = first.strip_suffix(b"\n").unwrap_or(&first);
    if first.is_empty() || read_at(first).is_some_and(|at| at >= now - HISTORY_KEPT - day) {
        return Ok(());
    }
    let bytes = std::fs::read(path).map_err(|e| AppError::io(path, e))?;
    let lines: Vec<(&[u8], Timestamp)> = bytes
        .split(|&byte| byte == b'\n')
        .filter_map(|line| Some((line, read_at(line)?)))
        .collect();
    // Counted from the newest state when the clock runs ahead of it, so a clock set wrong does
    // not throw the history away.
    let newest = lines.iter().map(|&(_, at)| at).max().unwrap_or(now);
    let cutoff = now.min(newest) - HISTORY_KEPT;
    let kept: Vec<u8> = lines
        .iter()
        .filter(|&&(_, at)| at >= cutoff)
        .flat_map(|&(line, _)| line.iter().chain(b"\n"))
        .copied()
        .collect();
    if kept.len() == bytes.len() {
        return Ok(());
    }
    files::replace(path, |file: &mut File| file.write_all(&kept))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

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
                    mode: OperationMode::Other(9),
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
        let read = || std::fs::read_to_string(&path).unwrap();

        write(&[line(90), line(10), line(1)]);
        prune_history(&path, now).unwrap();
        assert_eq!(
            read().lines().count(),
            3,
            "the oldest is not a day past 90 days yet"
        );

        write(&[line(95), line(93), "not json".into(), line(10), line(1)]);
        prune_history(&path, now).unwrap();
        assert_eq!(read(), [line(10), line(1)].join("\n") + "\n");

        std::fs::write(&path, "").unwrap();
        prune_history(&path, now).unwrap();
        assert_eq!(read(), "");
        std::fs::remove_file(&path).unwrap();
        prune_history(&path, now).unwrap();
    }

    #[test]
    fn a_clock_far_ahead_does_not_empty_the_history() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::under(home.path());
        let state: melcloud::AtaState =
            serde_json::from_str(include_str!("../../melcloud/tests/data/device-get.json"))
                .unwrap();
        let contact = state.view().last_communication.unwrap();
        let lines = format!(
            "{}\n{}\n",
            json!({"at": contact - SignedDuration::from_hours(24 * 60), "device": 1}),
            json!({"at": contact, "device": 1}),
        );
        std::fs::create_dir_all(paths.history().parent().unwrap()).unwrap();
        std::fs::write(paths.history(), &lines).unwrap();

        // Two reads, as a TUI makes them, with the clock 200 days ahead of MELCloud's.
        let wrong = contact + SignedDuration::from_hours(24 * 200);
        let later = SignedDuration::from_mins(10);
        record_history(&paths, state.view(), wrong, contact).unwrap();
        record_history(&paths, state.view(), wrong + later, contact + later).unwrap();
        let text = std::fs::read_to_string(paths.history()).unwrap();
        assert!(text.starts_with(&lines), "{text}");
        assert_eq!(text.lines().count(), 4);
    }

    #[test]
    fn a_line_cut_short_does_not_swallow_the_next() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("history.jsonl");
        std::fs::write(&path, "{\"at\":\"2026-01-15T09:00:00Z\",\"pow").unwrap();
        append_json_line(&path, &json!({"at": "2026-01-15T09:10:00Z"})).unwrap();
        append_json_line(&path, &json!({"at": "2026-01-15T09:20:00Z"})).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(lines[1], r#"{"at":"2026-01-15T09:10:00Z"}"#);
    }

    #[test]
    fn a_damaged_line_costs_only_itself() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::under(home.path());
        std::fs::create_dir_all(paths.history().parent().unwrap()).unwrap();
        let good = r#"{"at":"2026-01-15T09:00:00Z","device":1000001,"power":true,"mode":"mode 11","target":22.0,"room":21.0}"#;
        let mut bytes = b"\xff\xfe torn\n".to_vec();
        bytes.extend_from_slice(good.as_bytes());
        bytes.extend_from_slice(b"\n{\"at\":\"2026-01-15T09:05:00Z\",\"room\":\xe2\x82\n");
        std::fs::write(paths.history(), &bytes).unwrap();
        let since = "2026-01-15T00:00:00Z".parse().unwrap();
        let samples = history(&paths, DeviceId(1_000_001), since);
        assert_eq!(samples.len(), 1);
        assert_eq!(
            samples[0].mode,
            OperationMode::Other(11),
            "unknown codes survive"
        );

        let much_later = "2026-06-01T00:00:00Z".parse().unwrap();
        prune_history(&paths.history(), much_later).unwrap();
        assert_eq!(
            std::fs::read_to_string(paths.history()).unwrap(),
            format!("{good}\n"),
            "damaged lines are dropped with the next pruning"
        );
    }
}
