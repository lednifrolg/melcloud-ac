//! Where klima keeps its files.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use melcloud::DeviceId;

use crate::error::AppError;

/// `<value>/klima` when the XDG variable holds an absolute path, else `<home>/<fallback>/klima`.
fn xdg(value: Option<OsString>, home: &Path, fallback: &str) -> PathBuf {
    value
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
        .join("klima")
}

/// Config, state and cache directories.
#[derive(Debug, Clone)]
pub struct Paths {
    config: PathBuf,
    state: PathBuf,
    cache: PathBuf,
}

impl Paths {
    /// XDG locations (`~/.config/klima`, `~/.local/state/klima`, `~/.cache/klima`), or all three
    /// under `KLIMA_HOME` when it is set.
    pub fn discover() -> Result<Self, AppError> {
        if let Some(home) = env::var_os("KLIMA_HOME") {
            return Ok(Self::under(Path::new(&home)));
        }
        let home = env::home_dir()
            .filter(|home| home.is_absolute())
            .ok_or_else(|| AppError::Usage("cannot find the home directory".into()))?;
        Ok(Self {
            config: xdg(env::var_os("XDG_CONFIG_HOME"), &home, ".config"),
            state: xdg(env::var_os("XDG_STATE_HOME"), &home, ".local/state"),
            cache: xdg(env::var_os("XDG_CACHE_HOME"), &home, ".cache"),
        })
    }

    pub fn under(home: &Path) -> Self {
        Self {
            config: home.join("config"),
            state: home.join("state"),
            cache: home.join("cache"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    pub fn session(&self) -> PathBuf {
        self.state.join("session.json")
    }

    pub fn ledger(&self) -> PathBuf {
        self.state.join("ledger.json")
    }

    pub fn ledger_lock(&self) -> PathBuf {
        self.state.join("ledger.lock")
    }

    pub fn history(&self) -> PathBuf {
        self.state.join("history.jsonl")
    }

    pub fn devices(&self) -> PathBuf {
        self.cache.join("devices.json")
    }

    pub fn state_of(&self, device: DeviceId) -> PathBuf {
        self.cache.join(format!("state-{device}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_variables_win_only_with_absolute_paths() {
        let home = Path::new("/home/user");
        assert_eq!(
            xdg(Some("/xdg/state".into()), home, ".local/state"),
            Path::new("/xdg/state/klima")
        );
        assert_eq!(
            xdg(Some("relative".into()), home, ".local/state"),
            Path::new("/home/user/.local/state/klima")
        );
        assert_eq!(
            xdg(None, home, ".cache"),
            Path::new("/home/user/.cache/klima")
        );
    }
}
