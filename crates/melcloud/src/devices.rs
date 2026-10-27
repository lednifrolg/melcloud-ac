//! Devices from `User/ListDevices`.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::caps::{Capabilities, Permissions};

/// Device identifier (`DeviceID`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub i64);

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Building identifier (`BuildingID`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BuildingId(pub i64);

impl fmt::Display for BuildingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Kind of device (`DeviceType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// Air conditioner (ATA), the only kind this client controls.
    AirToAir,
    /// Air-to-water heat pump (ATW).
    AirToWater,
    /// Energy recovery ventilation (ERV).
    Ventilation,
    /// A code this client does not know.
    Other(i32),
}

impl DeviceKind {
    fn from_code(code: i32) -> Self {
        match code {
            0 => Self::AirToAir,
            1 => Self::AirToWater,
            3 => Self::Ventilation,
            other => Self::Other(other),
        }
    }
}

/// `AccessLevel` of the device owner; lower levels are guest access.
pub const OWNER_ACCESS_LEVEL: i32 = 4;

/// A device on the account, wherever it sits in the building structure.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Device {
    pub id: DeviceId,
    pub building_id: BuildingId,
    pub building_name: String,
    pub name: String,
    pub kind: DeviceKind,
    pub access_level: i32,
    pub permissions: Permissions,
    /// Model name of the first indoor unit.
    pub model: Option<String>,
    /// What the unit supports; only for air-to-air units.
    pub capabilities: Option<Capabilities>,
    /// The cloud delivers commands to the adapter at once (MQTT) instead of at its next
    /// check-in.
    pub instant_delivery: bool,
    pub wifi_signal_dbm: Option<i32>,
    /// Energy meter reading (`CurrentEnergyConsumed`). The unit is not verified; it looks like
    /// watt-hours.
    pub energy_meter: Option<i64>,
}

impl Device {
    pub fn is_owner(&self) -> bool {
        self.access_level == OWNER_ACCESS_LEVEL
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawBuilding {
    #[serde(rename = "ID")]
    id: BuildingId,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cooling_disabled: bool,
    structure: RawStructure,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawStructure {
    #[serde(default)]
    floors: Vec<RawFloor>,
    #[serde(default)]
    areas: Vec<RawArea>,
    #[serde(default)]
    devices: Vec<RawEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawFloor {
    #[serde(default)]
    areas: Vec<RawArea>,
    #[serde(default)]
    devices: Vec<RawEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawArea {
    #[serde(default)]
    devices: Vec<RawEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawEntry {
    #[serde(rename = "DeviceID")]
    device_id: DeviceId,
    device_name: String,
    access_level: i32,
    #[serde(default)]
    hide_dry_mode_control: bool,
    #[serde(default)]
    hide_vane_controls: bool,
    permissions: Permissions,
    device: RawDeviceInfo,
}

/// Model and adapter details of a device entry (`Device`). Fields missing for other device kinds
/// fall back to defaults so one heat pump on the account does not break the listing.
#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the API's boolean fields"
)]
pub(crate) struct RawDeviceInfo {
    pub(crate) device_type: i32,
    pub(crate) can_cool: bool,
    pub(crate) can_heat: bool,
    pub(crate) can_dry: bool,
    pub(crate) has_automatic_fan_speed: bool,
    pub(crate) number_of_fan_speeds: i32,
    pub(crate) model_supports_fan_speed: bool,
    pub(crate) model_supports_auto: bool,
    pub(crate) model_supports_heat: bool,
    pub(crate) model_supports_dry: bool,
    pub(crate) model_supports_vane_vertical: bool,
    pub(crate) model_supports_vane_horizontal: bool,
    pub(crate) model_supports_wide_vane: bool,
    pub(crate) temperature_increment: f64,
    pub(crate) min_temp_cool_dry: f64,
    pub(crate) max_temp_cool_dry: f64,
    pub(crate) min_temp_heat: f64,
    pub(crate) max_temp_heat: f64,
    pub(crate) min_temp_automatic: f64,
    pub(crate) max_temp_automatic: f64,
    mqtt_flags: i32,
    wifi_signal_strength: Option<i32>,
    has_energy_consumed_meter: bool,
    current_energy_consumed: Option<i64>,
    units: Vec<RawUnit>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct RawUnit {
    model: Option<String>,
    is_indoor: bool,
}

impl Device {
    fn from_raw(entry: &RawEntry, building: &RawBuilding) -> Self {
        let info = &entry.device;
        let kind = DeviceKind::from_code(info.device_type);
        let capabilities = (kind == DeviceKind::AirToAir).then(|| {
            Capabilities::from_model(
                info,
                entry.hide_dry_mode_control,
                entry.hide_vane_controls,
                building.cooling_disabled,
            )
        });
        Self {
            id: entry.device_id,
            building_id: building.id,
            building_name: building.name.clone().unwrap_or_default(),
            name: entry.device_name.clone(),
            kind,
            access_level: entry.access_level,
            permissions: entry.permissions,
            model: info
                .units
                .iter()
                .find(|u| u.is_indoor)
                .and_then(|u| u.model.clone()),
            capabilities,
            // The app checks `MqttFlags & 10 == 8`.
            instant_delivery: info.mqtt_flags & 0b1010 == 0b1000,
            wifi_signal_dbm: info.wifi_signal_strength,
            energy_meter: info
                .has_energy_consumed_meter
                .then_some(info.current_energy_consumed)
                .flatten(),
        }
    }
}

/// Devices from `User/ListDevices`: every device on every level of every building.
///
/// The response is kept exactly as the server sent it, so the list can be stored and read back
/// without depending on this crate's types.
#[derive(Clone, PartialEq)]
pub struct DeviceList {
    raw: Value,
    devices: Vec<Device>,
}

impl DeviceList {
    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    pub fn into_devices(self) -> Vec<Device> {
        self.devices
    }

    /// The response as the server sent it.
    pub fn raw(&self) -> &Value {
        &self.raw
    }

    fn from_raw(raw: Value) -> Result<Self, serde_json::Error> {
        let devices = devices_in(&Vec::<RawBuilding>::deserialize(&raw)?);
        Ok(Self { raw, devices })
    }
}

impl fmt::Debug for DeviceList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceList")
            .field("devices", &self.devices)
            .finish_non_exhaustive()
    }
}

impl Serialize for DeviceList {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DeviceList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_raw(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

fn devices_in(buildings: &[RawBuilding]) -> Vec<Device> {
    let mut devices = Vec::new();
    for building in buildings {
        let structure = &building.structure;
        let in_floors = structure.floors.iter().flat_map(|floor| {
            floor
                .devices
                .iter()
                .chain(floor.areas.iter().flat_map(|a| &a.devices))
        });
        let entries = structure
            .devices
            .iter()
            .chain(structure.areas.iter().flat_map(|area| &area.devices))
            .chain(in_floors);
        devices.extend(entries.map(|entry| Device::from_raw(entry, building)));
    }
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_DEVICES: &str = include_str!("../tests/data/list-devices.json");

    #[test]
    fn finds_devices_on_every_level() {
        let devices = serde_json::from_str::<DeviceList>(LIST_DEVICES)
            .unwrap()
            .into_devices();
        let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Living room", "Bedroom"]);
        assert!(
            devices
                .iter()
                .all(|d| d.building_id == BuildingId(2_000_001))
        );
        assert!(devices.iter().all(|d| d.building_name == "Example Home"));
    }

    #[test]
    fn reads_device_details() {
        let devices = serde_json::from_str::<DeviceList>(LIST_DEVICES)
            .unwrap()
            .into_devices();
        let living = &devices[0];
        assert_eq!(living.id, DeviceId(1_000_001));
        assert_eq!(living.kind, DeviceKind::AirToAir);
        assert!(living.is_owner());
        assert_eq!(living.model.as_deref(), Some("EXAMPLE-35"));
        assert!(living.instant_delivery);
        assert_eq!(living.wifi_signal_dbm, Some(-60));
        assert_eq!(living.energy_meter, Some(125_000));
        assert!(living.capabilities.is_some());

        let bedroom = &devices[1];
        assert!(!bedroom.is_owner());
        assert!(!bedroom.instant_delivery);
        assert_eq!(bedroom.energy_meter, None);
        assert!(!bedroom.permissions.can_set_fan_speed);
    }

    #[test]
    fn list_serializes_back_to_the_response() {
        let original: Value = serde_json::from_str(LIST_DEVICES).unwrap();
        let list: DeviceList = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(serde_json::to_value(&list).unwrap(), original);
    }

    #[test]
    fn other_device_kinds_have_no_capabilities() {
        let mut value: serde_json::Value = serde_json::from_str(LIST_DEVICES).unwrap();
        value[0]["Structure"]["Devices"][0]["Device"] = serde_json::json!({ "DeviceType": 1 });
        let devices = serde_json::from_value::<DeviceList>(value)
            .unwrap()
            .into_devices();
        assert_eq!(devices[0].kind, DeviceKind::AirToWater);
        assert_eq!(devices[0].capabilities, None);
    }
}
