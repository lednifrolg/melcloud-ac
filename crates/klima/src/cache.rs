//! Last known server responses, and the history of unit states.

use std::path::Path;

use jiff::{SignedDuration, Timestamp};
use melcloud::{AtaView, DeviceId, WeatherKind};
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
struct Sample<'a> {
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

/// Appends a state read from the server to the history used for charts.
pub fn record_history(paths: &Paths, view: &AtaView, now: Timestamp) -> Result<(), AppError> {
    let sample = Sample {
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
    append_json_line(&paths.history(), &sample)
}
