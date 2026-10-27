//! Validated commands for air-to-air units.

use std::fmt;

use serde_json::{Map, Value};

use crate::ata::{AtaState, FanSpeed, OperationMode, VaneHorizontal, VaneVertical};
use crate::caps::Capabilities;
use crate::devices::{Device, DeviceId};
use crate::flags::EffectiveFlags;

/// Something the account needs a permission for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Power,
    Mode,
    FanSpeed,
    Vanes,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Power => "switch the power",
            Self::Mode => "change the mode",
            Self::FanSpeed => "change the fan speed",
            Self::Vanes => "move the vanes",
        })
    }
}

/// Why a [`ChangeSet`] cannot be sent to a unit.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ChangeError {
    #[error("nothing to change")]
    Empty,
    #[error("{0} is not an air conditioner")]
    NotAirToAir(String),
    #[error("the state belongs to device {state}, not to device {device}")]
    DeviceMismatch { device: DeviceId, state: DeviceId },
    #[error("this account is not allowed to {0}")]
    NotPermitted(Action),
    #[error("{0} mode is not supported by this unit")]
    ModeNotSupported(OperationMode),
    #[error("{value:.1} °C is outside {min}-{max} °C for {mode} mode")]
    TemperatureOutOfRange {
        value: f64,
        mode: OperationMode,
        min: f64,
        max: f64,
    },
    #[error("{value} °C is not a multiple of the {step} °C step")]
    TemperatureStep { value: f64, step: f64 },
    #[error("fan speed {0} is not supported by this unit")]
    FanSpeedNotSupported(FanSpeed),
    #[error("vertical vane position {0} is not supported by this unit")]
    VaneVerticalNotSupported(VaneVertical),
    #[error("horizontal vane position {0} is not supported by this unit")]
    VaneHorizontalNotSupported(VaneHorizontal),
}

/// Changes to send to an air-to-air unit as one command. Unset fields stay as they are.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ChangeSet {
    pub power: Option<bool>,
    pub mode: Option<OperationMode>,
    /// Target temperature in °C.
    pub target_temperature: Option<f64>,
    pub fan_speed: Option<FanSpeed>,
    pub vane_vertical: Option<VaneVertical>,
    pub vane_horizontal: Option<VaneHorizontal>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Adds `later` on top of these changes; fields set in `later` win.
    pub fn merge(&mut self, later: Self) {
        self.power = later.power.or(self.power);
        self.mode = later.mode.or(self.mode);
        self.target_temperature = later.target_temperature.or(self.target_temperature);
        self.fan_speed = later.fan_speed.or(self.fan_speed);
        self.vane_vertical = later.vane_vertical.or(self.vane_vertical);
        self.vane_horizontal = later.vane_horizontal.or(self.vane_horizontal);
    }

    /// Checks the changes against what `device` supports and allows, and builds the command
    /// from its current `state`.
    pub fn build(&self, device: &Device, state: &AtaState) -> Result<SetAtaRequest, ChangeError> {
        if self.is_empty() {
            return Err(ChangeError::Empty);
        }
        let caps = device
            .capabilities
            .as_ref()
            .ok_or_else(|| ChangeError::NotAirToAir(device.name.clone()))?;
        let view = state.view();
        if view.device_id != device.id {
            return Err(ChangeError::DeviceMismatch {
                device: device.id,
                state: view.device_id,
            });
        }
        let allowed = device.permissions;
        let mut body = state.raw().clone();
        let mut flags = EffectiveFlags::empty();
        let mut set = |key: &str, value: Value, flag| {
            body.insert(key.to_owned(), value);
            flags |= flag;
        };

        if let Some(power) = self.power {
            permit(allowed.can_set_power, Action::Power)?;
            set("Power", power.into(), EffectiveFlags::POWER);
        }
        if let Some(mode) = self.mode {
            permit(allowed.can_set_operation_mode, Action::Mode)?;
            if !caps.supports_mode(mode) {
                return Err(ChangeError::ModeNotSupported(mode));
            }
            set(
                "OperationMode",
                mode.code().into(),
                EffectiveFlags::OPERATION_MODE,
            );
        }
        if let Some(celsius) = self.target_temperature {
            check_temperature(celsius, self.mode.unwrap_or(view.mode), caps)?;
            set(
                "SetTemperature",
                celsius.into(),
                EffectiveFlags::SET_TEMPERATURE,
            );
        }
        if let Some(fan) = self.fan_speed {
            permit(allowed.can_set_fan_speed, Action::FanSpeed)?;
            let supported = match fan {
                FanSpeed::Auto => caps.fan_auto,
                FanSpeed::Level(level) => (1..=caps.fan_levels).contains(&level),
                FanSpeed::Other(_) => false,
            };
            if !supported {
                return Err(ChangeError::FanSpeedNotSupported(fan));
            }
            set("SetFanSpeed", fan.code().into(), EffectiveFlags::FAN_SPEED);
        }
        if let Some(vane) = self.vane_vertical {
            permit(allowed.can_set_vane_direction, Action::Vanes)?;
            if !caps.vane_vertical || matches!(vane, VaneVertical::Other(_)) {
                return Err(ChangeError::VaneVerticalNotSupported(vane));
            }
            set(
                "VaneVertical",
                vane.code().into(),
                EffectiveFlags::VANE_VERTICAL,
            );
        }
        if let Some(vane) = self.vane_horizontal {
            permit(allowed.can_set_vane_direction, Action::Vanes)?;
            let supported = caps.vane_horizontal
                && match vane {
                    VaneHorizontal::Split => caps.wide_vane,
                    VaneHorizontal::Other(_) => false,
                    _ => true,
                };
            if !supported {
                return Err(ChangeError::VaneHorizontalNotSupported(vane));
            }
            set(
                "VaneHorizontal",
                vane.code().into(),
                EffectiveFlags::VANE_HORIZONTAL,
            );
        }

        body.insert("EffectiveFlags".to_owned(), flags.bits().into());
        Ok(SetAtaRequest {
            device_id: device.id,
            flags,
            body,
        })
    }
}

fn permit(allowed: bool, action: Action) -> Result<(), ChangeError> {
    if allowed {
        Ok(())
    } else {
        Err(ChangeError::NotPermitted(action))
    }
}

fn check_temperature(
    celsius: f64,
    mode: OperationMode,
    caps: &Capabilities,
) -> Result<(), ChangeError> {
    let range = caps.temperature_range(mode);
    if !range.contains(celsius) {
        return Err(ChangeError::TemperatureOutOfRange {
            value: celsius,
            mode,
            min: range.min,
            max: range.max,
        });
    }
    let step = caps.temperature_step;
    let steps = celsius / step;
    if step > 0.0 && (steps - steps.round()).abs() > 1e-6 {
        return Err(ChangeError::TemperatureStep {
            value: celsius,
            step,
        });
    }
    Ok(())
}

/// A validated `Device/SetAta` command: the unit's last state with the changed fields and the
/// matching [`EffectiveFlags`].
#[derive(Debug, Clone, PartialEq)]
pub struct SetAtaRequest {
    device_id: DeviceId,
    flags: EffectiveFlags,
    body: Map<String, Value>,
}

impl SetAtaRequest {
    pub fn device_id(&self) -> DeviceId {
        self.device_id
    }

    pub fn flags(&self) -> EffectiveFlags {
        self.flags
    }

    /// The JSON body that will be sent.
    pub fn body(&self) -> &Map<String, Value> {
        &self.body
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::devices::DeviceList;

    const LIST_DEVICES: &str = include_str!("../tests/data/list-devices.json");
    const DEVICE_GET: &str = include_str!("../tests/data/device-get.json");

    fn devices() -> Vec<Device> {
        serde_json::from_str::<DeviceList>(LIST_DEVICES)
            .unwrap()
            .into_devices()
    }

    fn owner() -> Device {
        devices().remove(0)
    }

    fn guest() -> Device {
        devices().remove(1)
    }

    fn state() -> AtaState {
        serde_json::from_str(DEVICE_GET).unwrap()
    }

    /// State of the guest unit: the synthetic state with its device ID.
    fn guest_state() -> AtaState {
        let mut raw: Value = serde_json::from_str(DEVICE_GET).unwrap();
        raw["DeviceID"] = json!(1_000_002);
        serde_json::from_value(raw).unwrap()
    }

    fn changes() -> ChangeSet {
        ChangeSet::default()
    }

    #[test]
    fn body_is_the_state_with_changes_and_flags() {
        let request = ChangeSet {
            target_temperature: Some(22.5),
            fan_speed: Some(FanSpeed::Auto),
            ..changes()
        }
        .build(&owner(), &state())
        .unwrap();
        assert_eq!(
            request.flags(),
            EffectiveFlags::SET_TEMPERATURE | EffectiveFlags::FAN_SPEED
        );

        let mut expected = state().raw().clone();
        expected.insert("SetTemperature".into(), json!(22.5));
        expected.insert("SetFanSpeed".into(), json!(0));
        expected.insert("EffectiveFlags".into(), json!(12));
        assert_eq!(request.body(), &expected);
    }

    #[test]
    fn every_field_maps_to_its_key_and_flag() {
        let request = ChangeSet {
            power: Some(false),
            mode: Some(OperationMode::Heat),
            target_temperature: Some(20.0),
            fan_speed: Some(FanSpeed::Level(5)),
            vane_vertical: Some(VaneVertical::Swing),
            vane_horizontal: Some(VaneHorizontal::Split),
        }
        .build(&owner(), &state())
        .unwrap();
        let body = request.body();
        assert_eq!(body["Power"], json!(false));
        assert_eq!(body["OperationMode"], json!(1));
        assert_eq!(body["SetTemperature"], json!(20.0));
        assert_eq!(body["SetFanSpeed"], json!(5));
        assert_eq!(body["VaneVertical"], json!(7));
        assert_eq!(body["VaneHorizontal"], json!(8));
        assert_eq!(body["EffectiveFlags"], json!(1 + 2 + 4 + 8 + 16 + 256));
    }

    #[test]
    fn empty_changes_are_rejected() {
        assert_eq!(changes().build(&owner(), &state()), Err(ChangeError::Empty));
    }

    #[test]
    fn state_must_belong_to_the_device() {
        let result = ChangeSet {
            power: Some(true),
            ..changes()
        }
        .build(&guest(), &state());
        assert_eq!(
            result,
            Err(ChangeError::DeviceMismatch {
                device: DeviceId(1_000_002),
                state: DeviceId(1_000_001)
            })
        );
    }

    #[test]
    fn temperature_follows_the_target_mode() {
        let in_cool = ChangeSet {
            target_temperature: Some(12.0),
            ..changes()
        };
        assert!(matches!(
            in_cool.build(&owner(), &state()),
            Err(ChangeError::TemperatureOutOfRange {
                mode: OperationMode::Cool,
                ..
            })
        ));
        let in_heat = ChangeSet {
            mode: Some(OperationMode::Heat),
            ..in_cool
        };
        assert!(in_heat.build(&owner(), &state()).is_ok());
    }

    #[test]
    fn temperature_must_match_the_step() {
        let result = ChangeSet {
            target_temperature: Some(22.3),
            ..changes()
        }
        .build(&owner(), &state());
        assert_eq!(
            result,
            Err(ChangeError::TemperatureStep {
                value: 22.3,
                step: 0.5
            })
        );
    }

    #[test]
    fn unsupported_settings_are_rejected() {
        let owner = owner();
        let state = state();
        let fan = ChangeSet {
            fan_speed: Some(FanSpeed::Level(6)),
            ..changes()
        };
        assert_eq!(
            fan.build(&owner, &state),
            Err(ChangeError::FanSpeedNotSupported(FanSpeed::Level(6)))
        );

        let guest = guest();
        let guest_state = guest_state();
        let dry = ChangeSet {
            mode: Some(OperationMode::Dry),
            ..changes()
        };
        assert_eq!(
            dry.build(&guest, &guest_state),
            Err(ChangeError::ModeNotSupported(OperationMode::Dry))
        );
    }

    #[test]
    fn missing_permissions_are_rejected() {
        let guest = guest();
        let state = guest_state();
        let fan = ChangeSet {
            fan_speed: Some(FanSpeed::Auto),
            ..changes()
        };
        assert_eq!(
            fan.build(&guest, &state),
            Err(ChangeError::NotPermitted(Action::FanSpeed))
        );
        let vane = ChangeSet {
            vane_vertical: Some(VaneVertical::Swing),
            ..changes()
        };
        assert_eq!(
            vane.build(&guest, &state),
            Err(ChangeError::NotPermitted(Action::Vanes))
        );
        let power = ChangeSet {
            power: Some(true),
            ..changes()
        };
        assert!(power.build(&guest, &state).is_ok());
    }

    #[test]
    fn later_changes_win_when_merged() {
        let mut merged = ChangeSet {
            target_temperature: Some(22.0),
            fan_speed: Some(FanSpeed::Auto),
            ..changes()
        };
        merged.merge(ChangeSet {
            target_temperature: Some(23.0),
            power: Some(true),
            ..changes()
        });
        assert_eq!(
            merged,
            ChangeSet {
                power: Some(true),
                target_temperature: Some(23.0),
                fan_speed: Some(FanSpeed::Auto),
                ..changes()
            }
        );
    }

    #[test]
    fn errors_read_well() {
        let error = ChangeError::TemperatureOutOfRange {
            value: 35.0,
            mode: OperationMode::Cool,
            min: 16.0,
            max: 31.0,
        };
        assert_eq!(
            error.to_string(),
            "35.0 °C is outside 16-31 °C for cool mode"
        );
        assert_eq!(
            ChangeError::NotPermitted(Action::FanSpeed).to_string(),
            "this account is not allowed to change the fan speed"
        );
    }
}
