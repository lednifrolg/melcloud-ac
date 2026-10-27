//! `config.toml`: optional settings.

use std::fs;
use std::io;

use serde::Deserialize;

use crate::error::AppError;
use crate::paths::Paths;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Email offered by `klima login`.
    pub email: Option<String>,
    /// Device name or ID used when `--device` is not given.
    pub device: Option<String>,
    /// Minutes between automatic refreshes in the TUI; at least 5.
    pub refresh_minutes: Option<u32>,
    /// Motion in the TUI: a spinning fan, swinging vanes, flashing digits. On by default.
    pub animations: Option<bool>,
}

/// Default and lowest refresh interval of the TUI, in minutes.
pub const REFRESH_MINUTES: u32 = 10;
pub const MIN_REFRESH_MINUTES: u32 = 5;

impl Config {
    pub fn animations(&self) -> bool {
        self.animations.unwrap_or(true)
    }

    pub fn refresh_minutes(&self) -> u32 {
        self.refresh_minutes
            .unwrap_or(REFRESH_MINUTES)
            .max(MIN_REFRESH_MINUTES)
    }
}

pub fn load(paths: &Paths) -> Result<Config, AppError> {
    let path = paths.config_file();
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| AppError::Config {
            path,
            message: e.to_string(),
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(AppError::io(path, e)),
    }
}
