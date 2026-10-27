//! Client for the classic MELCloud API (`app.melcloud.com`) used by Mitsubishi Electric air
//! conditioners.
//!
//! The API is unofficial and undocumented. Every request reaches a real account and every
//! command changes a real unit, so this crate never retries logins and treats HTTP 429 as a hard
//! stop. It has no UI dependencies; the `klima` binary builds the CLI and TUI on top of it.

/// Base URL of the classic MELCloud API.
pub const BASE_URL: &str = "https://app.melcloud.com/Mitsubishi.Wifi.Client";

/// App version sent in the login body. The server rejects versions it no longer supports.
pub const APP_VERSION: &str = "1.39.0.0";
