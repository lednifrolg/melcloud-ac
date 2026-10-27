//! What an air-to-air unit supports and what the account may change on it.

use serde::Deserialize;

use crate::ata::OperationMode;
use crate::devices::RawDeviceInfo;

/// What the account may change on a device (`Permissions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the API's boolean fields"
)]
pub struct Permissions {
    pub can_set_power: bool,
    pub can_set_operation_mode: bool,
    pub can_set_fan_speed: bool,
    pub can_set_vane_direction: bool,
}

/// Allowed target temperatures in °C, inclusive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemperatureRange {
    pub min: f64,
    pub max: f64,
}

impl TemperatureRange {
    pub fn contains(self, celsius: f64) -> bool {
        (self.min..=self.max).contains(&celsius)
    }
}

/// What an air-to-air unit supports, derived the way the official app does it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per optional feature"
)]
pub struct Capabilities {
    /// Modes the unit offers, in the order the official app lists them.
    pub modes: Vec<OperationMode>,
    /// Number of fan speed levels; 0 when the fan speed cannot be set.
    pub fan_levels: u8,
    pub fan_auto: bool,
    pub vane_vertical: bool,
    pub vane_horizontal: bool,
    /// Horizontal vanes can split to both sides.
    pub wide_vane: bool,
    /// Target temperatures are multiples of this, in °C.
    pub temperature_step: f64,
    pub heat_range: TemperatureRange,
    /// Range for cooling and drying.
    pub cool_range: TemperatureRange,
    pub auto_range: TemperatureRange,
}

impl Capabilities {
    pub fn supports_mode(&self, mode: OperationMode) -> bool {
        self.modes.contains(&mode)
    }

    /// Allowed target temperatures in `mode`. Modes without their own range (fan) use the
    /// widest range of all modes.
    pub fn temperature_range(&self, mode: OperationMode) -> TemperatureRange {
        match mode {
            OperationMode::Heat => self.heat_range,
            OperationMode::Dry | OperationMode::Cool => self.cool_range,
            OperationMode::Auto => self.auto_range,
            OperationMode::Fan | OperationMode::Other(_) => {
                let ranges = [self.heat_range, self.cool_range, self.auto_range];
                TemperatureRange {
                    min: ranges.iter().map(|r| r.min).fold(f64::INFINITY, f64::min),
                    max: ranges
                        .iter()
                        .map(|r| r.max)
                        .fold(f64::NEG_INFINITY, f64::max),
                }
            }
        }
    }

    pub(crate) fn from_model(
        info: &RawDeviceInfo,
        hide_dry: bool,
        hide_vanes: bool,
        cooling_disabled: bool,
    ) -> Self {
        let cool = info.can_cool && !cooling_disabled;
        let offered = |mode| match mode {
            OperationMode::Heat => info.model_supports_heat && info.can_heat,
            OperationMode::Dry => info.model_supports_dry && info.can_dry && !hide_dry,
            OperationMode::Cool => cool,
            // The app offers fan mode on every unit.
            OperationMode::Fan => true,
            OperationMode::Auto => info.model_supports_auto && info.can_heat && cool,
            OperationMode::Other(_) => false,
        };
        let fan_levels = if info.model_supports_fan_speed {
            u8::try_from(info.number_of_fan_speeds).unwrap_or(0)
        } else {
            0
        };
        Self {
            modes: OperationMode::KNOWN
                .into_iter()
                .filter(|&m| offered(m))
                .collect(),
            fan_levels,
            fan_auto: info.has_automatic_fan_speed,
            vane_vertical: info.model_supports_vane_vertical && !hide_vanes,
            vane_horizontal: info.model_supports_vane_horizontal && !hide_vanes,
            wide_vane: info.model_supports_wide_vane,
            temperature_step: info.temperature_increment,
            heat_range: TemperatureRange {
                min: info.min_temp_heat,
                max: info.max_temp_heat,
            },
            cool_range: TemperatureRange {
                min: info.min_temp_cool_dry,
                max: info.max_temp_cool_dry,
            },
            auto_range: TemperatureRange {
                min: info.min_temp_automatic,
                max: info.max_temp_automatic,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::devices::DeviceList;

    const LIST_DEVICES: &str = include_str!("../tests/data/list-devices.json");

    fn capabilities(list: &Value) -> Vec<Capabilities> {
        serde_json::from_value::<DeviceList>(list.clone())
            .unwrap()
            .into_devices()
            .into_iter()
            .map(|d| d.capabilities.unwrap())
            .collect()
    }

    fn synthetic() -> Value {
        serde_json::from_str(LIST_DEVICES).unwrap()
    }

    #[test]
    fn full_featured_unit() {
        use OperationMode::{Auto, Cool, Dry, Fan, Heat};
        let caps = &capabilities(&synthetic())[0];
        assert_eq!(caps.modes, [Heat, Dry, Cool, Fan, Auto]);
        assert_eq!(caps.fan_levels, 5);
        assert!(caps.fan_auto);
        assert!(caps.vane_vertical && caps.vane_horizontal && caps.wide_vane);
        assert_eq!(
            caps.temperature_range(Heat),
            TemperatureRange {
                min: 10.0,
                max: 31.0
            }
        );
        assert_eq!(
            caps.temperature_range(Dry),
            TemperatureRange {
                min: 16.0,
                max: 31.0
            }
        );
        assert_eq!(
            caps.temperature_range(Fan),
            TemperatureRange {
                min: 10.0,
                max: 31.0
            }
        );
    }

    #[test]
    fn limited_unit() {
        use OperationMode::{Cool, Fan, Heat};
        let caps = &capabilities(&synthetic())[1];
        assert_eq!(caps.modes, [Heat, Cool, Fan]);
        assert_eq!(caps.fan_levels, 4);
        assert!(caps.vane_vertical);
        assert!(!caps.vane_horizontal && !caps.wide_vane);
        assert_eq!(
            caps.temperature_range(Fan),
            TemperatureRange {
                min: 16.0,
                max: 30.0
            }
        );
    }

    #[test]
    fn building_without_cooling_drops_cool_and_auto() {
        use OperationMode::{Dry, Fan, Heat};
        let mut list = synthetic();
        list[0]["CoolingDisabled"] = json!(true);
        assert_eq!(capabilities(&list)[0].modes, [Heat, Dry, Fan]);
    }

    #[test]
    fn hidden_controls_are_not_offered() {
        let mut list = synthetic();
        let entry = &mut list[0]["Structure"]["Devices"][0];
        entry["HideDryModeControl"] = json!(true);
        entry["HideVaneControls"] = json!(true);
        let caps = &capabilities(&list)[0];
        assert!(!caps.supports_mode(OperationMode::Dry));
        assert!(!caps.vane_vertical && !caps.vane_horizontal);
    }

    #[test]
    fn range_bounds_are_inclusive() {
        let range = TemperatureRange {
            min: 16.0,
            max: 31.0,
        };
        assert!(range.contains(16.0) && range.contains(31.0));
        assert!(!range.contains(15.5) && !range.contains(31.5) && !range.contains(f64::NAN));
    }
}
