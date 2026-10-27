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
