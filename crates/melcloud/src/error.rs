//! Errors from talking to MELCloud.

use crate::APP_VERSION;

/// A request to MELCloud failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// HTTP 401: the session expired or is invalid. Log in again; nothing is retried.
    #[error("the MELCloud session expired or is invalid; log in again")]
    Unauthorized,
    /// HTTP 403.
    #[error("MELCloud refused access to this resource")]
    Forbidden,
    /// HTTP 429: the account is throttled for hours. Stop sending requests.
    #[error("MELCloud throttled this account (HTTP 429); requests must stop for a few hours")]
    Throttled,
    /// The login was rejected.
    #[error(transparent)]
    Login(#[from] LoginError),
    /// Any other unsuccessful HTTP status. `error_id` is the server's correlation ID
    /// (`X-Mitsubishi-ErrorId`), if it sent one.
    #[error("MELCloud answered HTTP {status}{}", error_id.as_ref().map(|id| format!(" (error ID {id})")).unwrap_or_default())]
    Status {
        status: u16,
        error_id: Option<String>,
    },
    /// The request did not complete: network, TLS or timeout.
    #[error("could not reach MELCloud: {0}")]
    Transport(#[from] reqwest::Error),
    /// The response did not have the expected shape.
    #[error("unexpected response from {endpoint}: {source}")]
    Decode {
        endpoint: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

/// Why MELCloud rejected a login (`ErrorId`, `LoginStatus`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LoginError {
    #[error("MELCloud terms are not available in the selected language")]
    TermsUnavailable,
    #[error("wrong email or password")]
    InvalidCredentials,
    #[error("the email address is not verified yet")]
    EmailNotVerified,
    #[error("the MELCloud account is blocked")]
    AccountBlocked,
    #[error("MELCloud sent a verification email; verify the address first")]
    VerificationEmailSent,
    #[error("MELCloud no longer accepts app version {APP_VERSION}")]
    AppVersionUnsupported,
    /// Too many wrong passwords.
    #[error("the account is locked after failed logins; try again in {minutes} minutes")]
    Locked { minutes: u32 },
    #[error("MELCloud wants a CAPTCHA; log in once in the official app or website")]
    CaptchaRequired,
    #[error("the MELCloud password has to be reset")]
    PasswordResetRequired,
    #[error(
        "MELCloud is under heavy load and wants a CAPTCHA; log in once in the official app or website"
    )]
    ServerBusy,
    #[error("MELCloud rejected the login with error {0}")]
    Other(i32),
    /// No session, and `LoginStatus` asks for email verification.
    #[error("MELCloud wants the email address verified first (login status {0})")]
    VerificationRequired(i32),
    #[error("MELCloud accepted the login but sent no session")]
    MissingSession,
}

impl LoginError {
    /// Maps `ErrorId`; `minutes` is `LoginMinutes`, used by the lockout error.
    pub(crate) fn from_error_id(error_id: i32, minutes: u32) -> Self {
        match error_id {
            0 => Self::TermsUnavailable,
            1 => Self::InvalidCredentials,
            2 => Self::EmailNotVerified,
            3 => Self::AccountBlocked,
            4 => Self::VerificationEmailSent,
            5 => Self::AppVersionUnsupported,
            6 => Self::Locked { minutes },
            7 => Self::CaptchaRequired,
            8 => Self::PasswordResetRequired,
            9 => Self::ServerBusy,
            other => Self::Other(other),
        }
    }
}
