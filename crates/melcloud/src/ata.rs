//! State of an air-to-air unit, as returned by `Device/Get` and `Device/SetAta`.

use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use jiff::civil::DateTime;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::devices::DeviceId;
use crate::flags::EffectiveFlags;
use crate::time::utc;

/// `ErrorCode` a unit reports when it has no error.
pub const NO_ERROR: i32 = 8000;

/// A value that does not name a known mode, fan speed or vane position.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} \"{value}\"; expected {expected}")]
pub struct ParseValueError {
    kind: &'static str,
    value: String,
    expected: &'static str,
}

impl ParseValueError {
    fn new(kind: &'static str, value: &str, expected: &'static str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
            expected,
        }
    }
}

/// Operating mode (`OperationMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationMode {
    Heat,
    Dry,
    Cool,
    Fan,
    Auto,
    /// A code this client does not know.
    Other(i32),
}

impl OperationMode {
    /// Known modes in the order the official app lists them.
    pub const KNOWN: [Self; 5] = [Self::Heat, Self::Dry, Self::Cool, Self::Fan, Self::Auto];

    pub fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Heat,
            2 => Self::Dry,
            3 => Self::Cool,
            7 => Self::Fan,
            8 => Self::Auto,
            other => Self::Other(other),
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Self::Heat => 1,
            Self::Dry => 2,
            Self::Cool => 3,
            Self::Fan => 7,
            Self::Auto => 8,
            Self::Other(code) => code,
        }
    }
}

impl fmt::Display for OperationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Heat => f.write_str("heat"),
            Self::Dry => f.write_str("dry"),
            Self::Cool => f.write_str("cool"),
            Self::Fan => f.write_str("fan"),
            Self::Auto => f.write_str("auto"),
            Self::Other(code) => write!(f, "mode {code}"),
        }
    }
}

impl FromStr for OperationMode {
    type Err = ParseValueError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::KNOWN
            .into_iter()
            .find(|mode| mode.to_string().eq_ignore_ascii_case(s))
            .ok_or_else(|| ParseValueError::new("mode", s, "heat, dry, cool, fan or auto"))
    }
}

/// Fan speed (`SetFanSpeed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FanSpeed {
    Auto,
    /// 1 up to the unit's number of fan speeds.
    Level(u8),
    /// A code this client does not know.
    Other(i32),
}

impl FanSpeed {
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Auto,
            level => u8::try_from(level).map_or(Self::Other(level), Self::Level),
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Self::Auto => 0,
            Self::Level(level) => i32::from(level),
            Self::Other(code) => code,
        }
    }
}

impl fmt::Display for FanSpeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Level(level) => write!(f, "{level}"),
            Self::Other(code) => write!(f, "speed code {code}"),
        }
    }
}

impl FromStr for FanSpeed {
    type Err = ParseValueError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        match s.parse::<u8>() {
            Ok(level) if level > 0 => Ok(Self::Level(level)),
            _ => Err(ParseValueError::new(
                "fan speed",
                s,
                "auto or a level from 1",
            )),
        }
    }
}

/// Vertical vane position (`VaneVertical`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VaneVertical {
    Auto,
    /// 1 to 5.
    Position(u8),
    Swing,
    /// A code this client does not know.
    Other(i32),
}

impl VaneVertical {
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Auto,
            1..=5 => Self::Position(u8::try_from(code).unwrap_or_default()),
            7 => Self::Swing,
            other => Self::Other(other),
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Self::Auto => 0,
            Self::Position(position) => i32::from(position),
            Self::Swing => 7,
            Self::Other(code) => code,
        }
    }
}

impl fmt::Display for VaneVertical {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Position(position) => write!(f, "{position}"),
            Self::Swing => f.write_str("swing"),
            Self::Other(code) => write!(f, "position code {code}"),
        }
    }
}

impl FromStr for VaneVertical {
    type Err = ParseValueError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "swing" => Ok(Self::Swing),
            other => match other.parse::<u8>() {
                Ok(position @ 1..=5) => Ok(Self::Position(position)),
                _ => Err(ParseValueError::new(
                    "vertical vane position",
                    s,
                    "auto, 1-5 or swing",
                )),
            },
        }
    }
}

/// Horizontal vane position (`VaneHorizontal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VaneHorizontal {
    Auto,
    /// 1 to 5.
    Position(u8),
    /// Both sides outwards; only on units with a wide vane.
    Split,
    Swing,
    /// A code this client does not know.
    Other(i32),
}

impl VaneHorizontal {
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Auto,
            1..=5 => Self::Position(u8::try_from(code).unwrap_or_default()),
            8 => Self::Split,
            12 => Self::Swing,
            other => Self::Other(other),
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Self::Auto => 0,
            Self::Position(position) => i32::from(position),
            Self::Split => 8,
            Self::Swing => 12,
            Self::Other(code) => code,
        }
    }
}

impl fmt::Display for VaneHorizontal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Position(position) => write!(f, "{position}"),
            Self::Split => f.write_str("split"),
            Self::Swing => f.write_str("swing"),
            Self::Other(code) => write!(f, "position code {code}"),
        }
    }
}

impl FromStr for VaneHorizontal {
    type Err = ParseValueError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "split" => Ok(Self::Split),
            "swing" => Ok(Self::Swing),
            other => match other.parse::<u8>() {
                Ok(position @ 1..=5) => Ok(Self::Position(position)),
                _ => Err(ParseValueError::new(
                    "horizontal vane position",
                    s,
                    "auto, 1-5, split or swing",
                )),
            },
        }
    }
}

/// Which part of the forecast a weather observation describes (`WeatherType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherKind {
    Current,
    Day,
    Night,
    /// A code this client does not know.
    Other(i32),
}

impl WeatherKind {
    fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Current,
            1 => Self::Day,
            2 => Self::Night,
            other => Self::Other(other),
        }
    }
}

/// Weather at the building's location, as reported with the unit state.
///
/// Times are in the building's local time zone, unlike the UTC state timestamps.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct WeatherObservation {
    pub kind: WeatherKind,
    pub time: DateTime,
    /// Degrees Celsius.
    pub temperature: f64,
    /// Percent.
    pub humidity: f64,
    pub condition: String,
    pub icon: String,
    pub sunrise: Option<DateTime>,
    pub sunset: Option<DateTime>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawWeather {
    date: DateTime,
    sunrise: Option<DateTime>,
    sunset: Option<DateTime>,
    temperature: f64,
    humidity: f64,
    condition_name: String,
    icon: String,
    weather_type: i32,
}

impl From<RawWeather> for WeatherObservation {
    fn from(raw: RawWeather) -> Self {
        Self {
            kind: WeatherKind::from_code(raw.weather_type),
            time: raw.date,
            temperature: raw.temperature,
            humidity: raw.humidity,
            condition: raw.condition_name,
            icon: raw.icon,
            sunrise: raw.sunrise,
            sunset: raw.sunset,
        }
    }
}

/// Typed, read-only view of an air-to-air unit's state.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AtaView {
    pub device_id: DeviceId,
    pub power: bool,
    pub mode: OperationMode,
    /// Target temperature in °C (`SetTemperature`).
    pub target_temperature: f64,
    /// Room temperature in °C.
    pub room_temperature: f64,
    pub fan_speed: FanSpeed,
    pub vane_vertical: VaneVertical,
    pub vane_horizontal: VaneHorizontal,
    /// Fields of the last command the unit has not applied yet; empty when settled.
    pub effective_flags: EffectiveFlags,
    /// The adapter missed its last check-in. Commands still wake it up, so this is not a reason
    /// to refuse control.
    pub offline: bool,
    /// Last time the adapter talked to the cloud.
    pub last_communication: Option<Timestamp>,
    /// Next time the adapter is expected to talk to the cloud.
    pub next_communication: Option<Timestamp>,
    /// Unit error code; [`NO_ERROR`] when there is none.
    pub error_code: i32,
    pub error_message: Option<String>,
    /// Weather entries that could not be read are left out.
    pub weather: Vec<WeatherObservation>,
}

impl AtaView {
    pub fn has_error(&self) -> bool {
        self.error_code != NO_ERROR
    }

    /// Whether this state proves that a command sent at `sent_at` was applied: the adapter
    /// talked to the cloud after the command and no fields are pending.
    pub fn confirms(&self, sent_at: Timestamp) -> bool {
        self.effective_flags.is_empty() && self.last_communication.is_some_and(|t| t > sent_at)
    }
}

fn no_error() -> i32 {
    NO_ERROR
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawAtaView {
    #[serde(rename = "DeviceID")]
    device_id: DeviceId,
    power: bool,
    operation_mode: i32,
    set_temperature: f64,
    room_temperature: f64,
    set_fan_speed: i32,
    vane_vertical: i32,
    vane_horizontal: i32,
    effective_flags: u64,
    #[serde(default)]
    offline: bool,
    last_communication: Option<DateTime>,
    next_communication: Option<DateTime>,
    #[serde(default = "no_error")]
    error_code: i32,
    error_message: Option<String>,
    #[serde(default)]
    weather_observations: Vec<Value>,
}

impl From<RawAtaView> for AtaView {
    fn from(raw: RawAtaView) -> Self {
        Self {
            device_id: raw.device_id,
            power: raw.power,
            mode: OperationMode::from_code(raw.operation_mode),
            target_temperature: raw.set_temperature,
            room_temperature: raw.room_temperature,
            fan_speed: FanSpeed::from_code(raw.set_fan_speed),
            vane_vertical: VaneVertical::from_code(raw.vane_vertical),
            vane_horizontal: VaneHorizontal::from_code(raw.vane_horizontal),
            effective_flags: EffectiveFlags::from_bits_retain(raw.effective_flags),
            offline: raw.offline,
            last_communication: raw.last_communication.and_then(utc),
            next_communication: raw.next_communication.and_then(utc),
            error_code: raw.error_code,
            error_message: raw.error_message,
            weather: raw
                .weather_observations
                .into_iter()
                .filter_map(|entry| serde_json::from_value::<RawWeather>(entry).ok())
                .map(WeatherObservation::from)
                .collect(),
        }
    }
}

/// State of an air-to-air unit exactly as the server sent it, with a typed view.
///
/// Commands send this object back with changed fields, as the official app does, so the raw
/// JSON is kept untouched, including fields this client does not know.
#[derive(Clone, PartialEq)]
pub struct AtaState {
    raw: Map<String, Value>,
    view: AtaView,
}

impl AtaState {
    pub fn view(&self) -> &AtaView {
        &self.view
    }

    /// The object as the server sent it.
    pub fn raw(&self) -> &Map<String, Value> {
        &self.raw
    }

    fn from_raw(raw: Map<String, Value>) -> Result<Self, serde_json::Error> {
        let view = serde_json::from_value::<RawAtaView>(Value::Object(raw.clone()))?.into();
        Ok(Self { raw, view })
    }
}

impl fmt::Debug for AtaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AtaState")
            .field("view", &self.view)
            .finish_non_exhaustive()
    }
}

impl Serialize for AtaState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AtaState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Map::deserialize(deserializer)?;
        Self::from_raw(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE_GET: &str = include_str!("../tests/data/device-get.json");

    fn state() -> AtaState {
        serde_json::from_str(DEVICE_GET).unwrap()
    }

    #[test]
    fn view_reads_the_state() {
        let state = state();
        let view = state.view();
        assert_eq!(view.device_id, DeviceId(1_000_001));
        assert!(view.power);
        assert_eq!(view.mode, OperationMode::Cool);
        assert!((view.target_temperature - 22.0).abs() < 1e-9);
        assert!((view.room_temperature - 24.5).abs() < 1e-9);
        assert_eq!(view.fan_speed, FanSpeed::Level(2));
        assert_eq!(view.vane_vertical, VaneVertical::Position(3));
        assert_eq!(view.vane_horizontal, VaneHorizontal::Position(3));
        assert!(view.effective_flags.is_empty());
        assert!(!view.has_error());
        assert_eq!(
            view.last_communication,
            Some("2026-01-15T10:05:00Z".parse().unwrap())
        );
        assert_eq!(view.weather.len(), 3);
        assert_eq!(view.weather[0].kind, WeatherKind::Current);
        assert_eq!(view.weather[2].kind, WeatherKind::Night);
    }

    #[test]
    fn raw_keeps_every_field() {
        let original: Map<String, Value> = serde_json::from_str(DEVICE_GET).unwrap();
        assert_eq!(state().raw(), &original);
    }

    #[test]
    fn state_serializes_back_to_the_response() {
        let original: Value = serde_json::from_str(DEVICE_GET).unwrap();
        assert_eq!(serde_json::to_value(state()).unwrap(), original);
    }

    #[test]
    fn unreadable_weather_entries_are_skipped() {
        let mut raw: Map<String, Value> = serde_json::from_str(DEVICE_GET).unwrap();
        raw["WeatherObservations"][0]["Temperature"] = Value::from("warm");
        let state: AtaState = serde_json::from_value(Value::Object(raw)).unwrap();
        assert_eq!(state.view().weather.len(), 2);
    }

    #[test]
    fn confirmation_needs_newer_contact_and_no_pending_flags() {
        let view = state().view().clone();
        let before = "2026-01-15T10:04:59Z".parse().unwrap();
        let after = "2026-01-15T10:05:01Z".parse().unwrap();
        assert!(view.confirms(before));
        assert!(!view.confirms(after));

        let mut pending = view;
        pending.effective_flags = EffectiveFlags::SET_TEMPERATURE;
        assert!(!pending.confirms(before));
    }

    #[test]
    fn codes_round_trip() {
        for code in [1, 2, 3, 7, 8, 5] {
            assert_eq!(OperationMode::from_code(code).code(), code);
        }
        for code in [0, 1, 5, -1] {
            assert_eq!(FanSpeed::from_code(code).code(), code);
        }
        for code in [0, 1, 5, 7, 6] {
            assert_eq!(VaneVertical::from_code(code).code(), code);
        }
        for code in [0, 3, 8, 12, 9] {
            assert_eq!(VaneHorizontal::from_code(code).code(), code);
        }
        assert_eq!(VaneHorizontal::from_code(8), VaneHorizontal::Split);
        assert_eq!(VaneVertical::from_code(6), VaneVertical::Other(6));
    }

    #[test]
    fn values_parse_from_text() {
        assert_eq!("COOL".parse(), Ok(OperationMode::Cool));
        assert!("ice".parse::<OperationMode>().is_err());
        assert_eq!("auto".parse(), Ok(FanSpeed::Auto));
        assert_eq!("3".parse(), Ok(FanSpeed::Level(3)));
        assert!("0".parse::<FanSpeed>().is_err());
        assert_eq!("swing".parse(), Ok(VaneVertical::Swing));
        assert!("6".parse::<VaneVertical>().is_err());
        assert_eq!("split".parse(), Ok(VaneHorizontal::Split));
        assert_eq!("5".parse(), Ok(VaneHorizontal::Position(5)));
    }
}
