//! Where klima keeps its files.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use melcloud::DeviceId;

use crate::error::AppError;

/// `<value>/klima` when the variable holds an absolute path, else `<home>/<fallback>/klima`.
fn base(value: Option<OsString>, home: &Path, fallback: &str) -> PathBuf {
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
    /// The usual locations of the system, or all three under `KLIMA_HOME` when it is set.
    pub fn discover() -> Result<Self, AppError> {
        if let Some(home) = env::var_os("KLIMA_HOME") {
            return Ok(Self::under(Path::new(&home)));
        }
        let home = env::home_dir()
            .filter(|home| home.is_absolute())
            .ok_or_else(|| AppError::Usage("cannot find the home directory".into()))?;
        Ok(Self::of_system(&home))
    }

    /// XDG locations: `~/.config/klima`, `~/.local/state/klima` and `~/.cache/klima`.
    #[cfg(not(windows))]
    fn of_system(home: &Path) -> Self {
        Self {
            config: base(env::var_os("XDG_CONFIG_HOME"), home, ".config"),
            state: base(env::var_os("XDG_STATE_HOME"), home, ".local/state"),
            cache: base(env::var_os("XDG_CACHE_HOME"), home, ".cache"),
        }
    }

    /// The config in `%APPDATA%\klima`, which may roam with the user; state and cache, which
    /// belong to this computer, in `%LOCALAPPDATA%\klima`.
    #[cfg(windows)]
    fn of_system(home: &Path) -> Self {
        let local = base(env::var_os("LOCALAPPDATA"), home, r"AppData\Local");
        Self {
            config: base(env::var_os("APPDATA"), home, r"AppData\Roaming"),
            state: local.join("state"),
            cache: local.join("cache"),
        }
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

    pub fn history_lock(&self) -> PathBuf {
        self.state.join("history.lock")
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
    fn variables_win_only_with_absolute_paths() {
        let root = Path::new(if cfg!(windows) { r"C:\" } else { "/" });
        let home = root.join("home");
        assert_eq!(
            base(Some(root.join("state").into()), &home, "fallback"),
            root.join("state/klima")
        );
        assert_eq!(
            base(Some("relative".into()), &home, "fallback"),
            home.join("fallback/klima")
        );
        assert_eq!(base(None, &home, "fallback"), home.join("fallback/klima"));
    }
}
