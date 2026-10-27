//! Client for the classic MELCloud API (`app.melcloud.com`) used by Mitsubishi Electric air
//! conditioners.
//!
//! The API is unofficial and undocumented. Every request reaches a real account and every
//! command changes a real unit, so this crate never retries logins and treats HTTP 429 as a hard
//! stop. It has no UI dependencies; the `klima` binary builds the CLI and TUI on top of it.
//!
//! A typical session: [`Client::login`], [`Client::devices`] to find a unit and its
//! [`Capabilities`], [`Client::ata_state`] to read it, then a [`ChangeSet`] built into a
//! [`SetAtaRequest`] for [`Client::set_ata`].

mod ata;
mod caps;
mod change;
mod client;
mod devices;
mod error;
mod flags;
mod text;
mod time;

pub use ata::{
    AtaState, AtaView, FanSpeed, NO_ERROR, OperationMode, ParseValueError, VaneHorizontal,
    VaneVertical, WeatherKind, WeatherObservation,
};
pub use caps::{Capabilities, Permissions, TemperatureRange};
pub use change::{Action, ChangeError, ChangeSet, SetAtaRequest};
pub use client::{Client, ContextKey, Session};
pub use devices::{BuildingId, Device, DeviceId, DeviceKind, DeviceList, OWNER_ACCESS_LEVEL};
pub use error::{Error, LoginError};
pub use flags::EffectiveFlags;

/// Base URL of the classic MELCloud API.
pub const BASE_URL: &str = "https://app.melcloud.com/Mitsubishi.Wifi.Client";

/// App version sent in the login body. The server rejects versions it no longer supports.
pub const APP_VERSION: &str = "1.39.0.0";
