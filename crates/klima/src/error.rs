//! Errors and their exit codes.

use std::path::PathBuf;
use std::process::ExitCode;

use jiff::Timestamp;
use melcloud::DeviceId;

use crate::governor::Refusal;
use crate::output::local_time;

/// Something stopped a command. Every variant maps to a documented exit code.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Invalid(melcloud::ChangeError),
    #[error(
        "the adapter did not confirm the command within {seconds} s; it may still apply it, \
         check later with `klima status --fresh`"
    )]
    NotConfirmed { seconds: u64 },
    #[error("the adapter took the command, but the unit reports {0}")]
    NotApplied(String),
    #[error("{path}: {message}")]
    Config { path: PathBuf, message: String },
    #[error("not logged in; run `klima login`")]
    NotLoggedIn,
    #[error("the MELCloud session expired; run `klima login`")]
    SessionExpired,
    #[error(
        "MELCloud throttled this account (HTTP 429); requests are paused until {}. \
         `klima unblock` lifts the pause, but the server keeps the account limited for hours",
        local_time(*until)
    )]
    Throttled { until: Timestamp },
    #[error("MELCloud locked logins after failed attempts; try again after {}", local_time(*until))]
    LoginLocked { until: Timestamp },
    #[error("{0}")]
    Refused(Refusal),
    #[error("login failed: {0}")]
    Login(melcloud::LoginError),
    #[error(transparent)]
    Api(melcloud::Error),
    #[error("MELCloud answered with the state of device {answered} instead of {asked}")]
    WrongDevice { asked: DeviceId, answered: DeviceId },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is unreadable ({source}); fix or delete it")]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// An error the output already explained; it sets the exit code without being repeated.
    #[error(transparent)]
    Shown(Box<AppError>),
}

impl AppError {
    /// 2 usage, 3 blocked by limits, 4 login needed, 5 command not confirmed, 1 anything else.
    pub fn exit_code(&self) -> ExitCode {
        ExitCode::from(match self {
            Self::Usage(_) | Self::Config { .. } | Self::Invalid(_) => 2,
            Self::Throttled { .. } | Self::LoginLocked { .. } | Self::Refused(_) => 3,
            Self::NotLoggedIn | Self::SessionExpired => 4,
            Self::NotConfirmed { .. } | Self::NotApplied(_) => 5,
            Self::Login(_)
            | Self::Api(_)
            | Self::WrongDevice { .. }
            | Self::Io { .. }
            | Self::Corrupt { .. } => 1,
            Self::Shown(error) => return error.exit_code(),
        })
    }

    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
