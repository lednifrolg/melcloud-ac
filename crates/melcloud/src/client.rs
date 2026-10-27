//! HTTP client for the classic MELCloud API.

use std::fmt;
use std::time::Duration;

use jiff::Timestamp;
use jiff::civil::DateTime;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::ata::AtaState;
use crate::change::SetAtaRequest;
use crate::devices::{BuildingId, Device, DeviceId, parse_devices};
use crate::error::{Error, LoginError};
use crate::time::utc;
use crate::{APP_VERSION, BASE_URL};

const LOGIN: &str = "/Login/ClientLogin3";
const LIST_DEVICES: &str = "/User/ListDevices";
const DEVICE_GET: &str = "/Device/Get";
const SET_ATA: &str = "/Device/SetAta";

const CONTEXT_KEY_HEADER: &str = "X-MitsContextKey";
const ERROR_ID_HEADER: &str = "X-Mitsubishi-ErrorId";
const JSON: &str = "application/json";
const JSON_BODY: &str = "application/json; charset=utf-8";
const TIMEOUT: Duration = Duration::from_secs(30);

/// Session token (`ContextKey`). It grants full access to the account, so it never shows up in
/// `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct ContextKey(String);

impl ContextKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The token itself, for storing the session.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ContextKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContextKey(<redacted>)")
    }
}

/// A logged-in session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    key: ContextKey,
    expires_at: Option<Timestamp>,
    use_fahrenheit: bool,
}

impl Session {
    /// Restores a session, e.g. one stored on disk.
    pub fn new(key: ContextKey, expires_at: Option<Timestamp>, use_fahrenheit: bool) -> Self {
        Self {
            key,
            expires_at,
            use_fahrenheit,
        }
    }

    pub fn key(&self) -> &ContextKey {
        &self.key
    }

    /// When the server will stop accepting the session, if it said so.
    pub fn expires_at(&self) -> Option<Timestamp> {
        self.expires_at
    }

    /// The user prefers Fahrenheit. The API itself always uses Celsius.
    pub fn use_fahrenheit(&self) -> bool {
        self.use_fahrenheit
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LoginResponse {
    error_id: Option<i32>,
    #[serde(default)]
    login_status: i32,
    #[serde(default)]
    login_minutes: i64,
    login_data: Option<LoginData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LoginData {
    context_key: String,
    expiry: Option<DateTime>,
    #[serde(default)]
    use_fahrenheit: bool,
}

impl LoginResponse {
    fn into_session(self) -> Result<Session, LoginError> {
        if let Some(error_id) = self.error_id {
            let minutes = u32::try_from(self.login_minutes).unwrap_or(0);
            return Err(LoginError::from_error_id(error_id, minutes));
        }
        match self.login_data {
            Some(data) => Ok(Session {
                key: ContextKey(data.context_key),
                expires_at: data.expiry.and_then(utc),
                use_fahrenheit: data.use_fahrenheit,
            }),
            None if matches!(self.login_status, 2 | 5) => {
                Err(LoginError::VerificationRequired(self.login_status))
            }
            None => Err(LoginError::MissingSession),
        }
    }
}

/// Client for the classic MELCloud API.
///
/// Every call is exactly one HTTP request. Nothing is retried: the API locks accounts after
/// repeated logins and throttles them for hours after too many requests, so retries are the
/// caller's decision.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
}

impl Client {
    /// A client for `app.melcloud.com` that identifies itself with `user_agent`.
    pub fn new(user_agent: &str) -> Result<Self, Error> {
        Self::with_base_url(user_agent, BASE_URL)
    }

    /// A client for another server, e.g. a test double.
    pub fn with_base_url(user_agent: &str, base_url: &str) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
        })
    }

    /// Logs in (`Login/ClientLogin3`). `persist` asks for a longer session, like "remember me"
    /// in the app.
    ///
    /// Never call this in a loop: wrong passwords lock the account behind a CAPTCHA.
    pub async fn login(
        &self,
        email: &str,
        password: &str,
        persist: bool,
    ) -> Result<Session, Error> {
        let body = json!({
            "Email": email,
            "Password": password,
            "Language": 0,
            "AppVersion": APP_VERSION,
            "Persist": persist,
            "CaptchaResponse": null,
        });
        let request = self
            .http
            .post(self.url(LOGIN))
            .header(CONTENT_TYPE, JSON_BODY)
            .body(body.to_string());
        let response: LoginResponse = self.call(LOGIN, request).await?;
        Ok(response.into_session()?)
    }

    /// Every device on the account (`User/ListDevices`).
    pub async fn devices(&self, session: &Session) -> Result<Vec<Device>, Error> {
        let request = authorized(self.http.get(self.url(LIST_DEVICES)), session);
        let body = self.send(request).await?;
        parse_devices(&body).map_err(|source| Error::Decode {
            endpoint: LIST_DEVICES,
            source,
        })
    }

    /// Current state of an air-to-air unit (`Device/Get`).
    pub async fn ata_state(
        &self,
        session: &Session,
        device: DeviceId,
        building: BuildingId,
    ) -> Result<AtaState, Error> {
        let url = format!("{}?id={device}&buildingID={building}", self.url(DEVICE_GET));
        let request = authorized(self.http.get(url), session);
        self.call(DEVICE_GET, request).await
    }

    /// Sends a command (`Device/SetAta`) and returns the state the server answers with. The
    /// unit applies it later; see [`crate::AtaView::confirms`].
    pub async fn set_ata(
        &self,
        session: &Session,
        command: &SetAtaRequest,
    ) -> Result<AtaState, Error> {
        let body = serde_json::to_vec(command.body()).expect("a JSON object always serializes");
        let request = authorized(self.http.post(self.url(SET_ATA)), session)
            .header(CONTENT_TYPE, JSON_BODY)
            .body(body);
        self.call(SET_ATA, request).await
    }

    fn url(&self, endpoint: &str) -> String {
        format!("{}{endpoint}", self.base_url)
    }

    async fn call<T: DeserializeOwned>(
        &self,
        endpoint: &'static str,
        request: RequestBuilder,
    ) -> Result<T, Error> {
        let body = self.send(request).await?;
        serde_json::from_slice(&body).map_err(|source| Error::Decode { endpoint, source })
    }

    async fn send(&self, request: RequestBuilder) -> Result<Vec<u8>, Error> {
        let response = request.header(ACCEPT, JSON).send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response.bytes().await?.to_vec());
        }
        Err(match status {
            StatusCode::UNAUTHORIZED => Error::Unauthorized,
            StatusCode::FORBIDDEN => Error::Forbidden,
            StatusCode::TOO_MANY_REQUESTS => Error::Throttled,
            _ => Error::Status {
                status: status.as_u16(),
                error_id: response
                    .headers()
                    .get(ERROR_ID_HEADER)
                    .and_then(|id| id.to_str().ok())
                    .map(str::to_owned),
            },
        })
    }
}

fn authorized(request: RequestBuilder, session: &Session) -> RequestBuilder {
    request.header(CONTEXT_KEY_HEADER, session.key.expose())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_key_stays_out_of_debug_output() {
        let session = Session::new(ContextKey::new("SECRET-KEY"), None, false);
        let debug = format!("{session:?}");
        assert!(!debug.contains("SECRET-KEY"), "{debug}");
        assert!(debug.contains("redacted"));
    }
}
