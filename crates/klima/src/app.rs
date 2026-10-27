//! Every MELCloud request goes through [`App`]: it asks the request budget first and records
//! what the server answered.

use std::env;
use std::future::Future;

use jiff::Timestamp;
use melcloud::{AtaState, Client, Device, DeviceList, LoginError};

use crate::cache::{self, Cached, DEVICES_TTL};
use crate::config::{self, Config};
use crate::error::AppError;
use crate::governor::{self, Request};
use crate::paths::Paths;
use crate::session::{self, Account};

/// Sent with every request (decision: an honest user agent).
pub const USER_AGENT: &str = concat!("klima/", env!("CARGO_PKG_VERSION"));

pub struct App {
    pub paths: Paths,
    pub config: Config,
    pub client: Client,
}

impl App {
    /// Uses `KLIMA_API_URL` instead of MELCloud when set, for tests against a mock server.
    pub fn new() -> Result<Self, AppError> {
        let paths = Paths::discover()?;
        let config = config::load(&paths)?;
        let client = match env::var("KLIMA_API_URL") {
            Ok(url) => Client::with_base_url(USER_AGENT, &url),
            Err(_) => Client::new(USER_AGENT),
        }
        .map_err(AppError::Api)?;
        Ok(Self {
            paths,
            config,
            client,
        })
    }

    /// The logged-in account, if its session has not expired.
    pub fn account(&self) -> Result<Account, AppError> {
        let account = session::load(&self.paths)?.ok_or(AppError::NotLoggedIn)?;
        if account.expired(Timestamp::now()) {
            session::delete(&self.paths)?;
            return Err(AppError::SessionExpired);
        }
        Ok(account)
    }

    /// Sends one request if the budget allows it and handles the answers that affect every
    /// later request.
    pub async fn call<T>(
        &self,
        request: Request,
        call: impl Future<Output = Result<T, melcloud::Error>>,
    ) -> Result<T, AppError> {
        governor::update(&self.paths, |ledger| {
            ledger.admit(request, Timestamp::now())
        })?
        .map_err(AppError::Refused)?;
        match call.await {
            Ok(value) => Ok(value),
            Err(melcloud::Error::Throttled) => {
                let until = governor::update(&self.paths, |l| l.throttled(Timestamp::now()))?;
                Err(AppError::Throttled { until })
            }
            Err(melcloud::Error::Unauthorized) => {
                session::delete(&self.paths)?;
                Err(AppError::SessionExpired)
            }
            Err(melcloud::Error::Login(LoginError::Locked { minutes })) => {
                let until =
                    governor::update(&self.paths, |l| l.login_locked(Timestamp::now(), minutes))?;
                Err(AppError::LoginLocked { until })
            }
            Err(melcloud::Error::Login(reason)) => Err(AppError::Login(reason)),
            Err(error) => Err(AppError::Api(error)),
        }
    }

    /// The device list: cached for a day unless `fresh`.
    pub async fn devices(&self, fresh: bool) -> Result<Cached<DeviceList>, AppError> {
        let path = self.paths.devices();
        let now = Timestamp::now();
        if !fresh
            && let Some(cached) =
                cache::load::<DeviceList>(&path).filter(|c| c.fresh(DEVICES_TTL, now))
        {
            return Ok(cached);
        }
        let account = self.account()?;
        let list = self
            .call(Request::Devices, self.client.devices(&account.session))
            .await?;
        cache::store(&path, &list, now)?;
        Ok(Cached {
            fetched_at: now,
            value: list,
        })
    }

    /// Reads the unit's state from the server and remembers it.
    pub async fn read_state(&self, device: &Device) -> Result<Cached<AtaState>, AppError> {
        let account = self.account()?;
        let state = self
            .call(
                Request::State(device.id),
                self.client
                    .ata_state(&account.session, device.id, device.building_id),
            )
            .await?;
        let answered = state.view().device_id;
        if answered != device.id {
            return Err(AppError::WrongDevice {
                asked: device.id,
                answered,
            });
        }
        let now = Timestamp::now();
        cache::store(&self.paths.state_of(device.id), &state, now)?;
        cache::record_history(&self.paths, state.view(), now)?;
        Ok(Cached {
            fetched_at: now,
            value: state,
        })
    }
}
