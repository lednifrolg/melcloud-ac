//! Every MELCloud request goes through [`App`]: it asks the request budget first and records
//! what the server answered.

use std::env;
use std::future::Future;

use jiff::Timestamp;
use melcloud::{AtaState, Client, Device, DeviceList, LoginError, SetAtaRequest};

use crate::cache::{self, Cached, DEVICES_TTL, STATE_TTL};
use crate::config::{self, Config};
use crate::error::AppError;
use crate::governor::{self, Refusal, Request};
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

    /// Logs in with the password and stores only the session.
    pub async fn login(&self, email: &str, password: &str) -> Result<melcloud::Session, AppError> {
        let session = self
            .call(Request::Login, self.client.login(email, password, true))
            .await?;
        session::save(&self.paths, email, &session, Timestamp::now())?;
        Ok(session)
    }

    /// The device list as last fetched, however old, or a fetched one when there is none yet.
    /// For what needs only names and IDs, which rarely change.
    pub async fn known_devices(&self) -> Result<Cached<DeviceList>, AppError> {
        match cache::load::<DeviceList>(&self.paths.devices()) {
            Some(cached) => Ok(cached),
            None => self.devices(true).await,
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

    /// Reads the unit's state from the server and remembers it. `request` is
    /// [`Request::State`] for an ordinary read or [`Request::Confirm`] after a command.
    pub async fn read_state(
        &self,
        device: &Device,
        request: Request,
    ) -> Result<Cached<AtaState>, AppError> {
        let account = self.account()?;
        let state = self
            .call(
                request,
                self.client
                    .ata_state(&account.session, device.id, device.building_id),
            )
            .await?;
        check_device(device, &state)?;
        let now = Timestamp::now();
        cache::store(&self.paths.state_of(device.id), &state, now)?;
        // MELCloud's clock keeps a local clock far ahead from making the whole history look
        // old. The history is a convenience: a read that worked stays a success without it.
        let reference = self
            .client
            .server_time()
            .map_or(now, |server| now.min(server));
        let _ = cache::record_history(&self.paths, state.view(), now, reference);
        Ok(Cached {
            fetched_at: now,
            value: state,
        })
    }

    /// The state a command is built on: the cached one while it is fresh, otherwise a new read.
    /// When that read is refused only because the unit was read moments ago, that recent state
    /// is used.
    pub async fn base_state(&self, device: &Device) -> Result<Cached<AtaState>, AppError> {
        let cached = cache::load::<AtaState>(&self.paths.state_of(device.id));
        match cached {
            Some(cached) if cached.fresh(STATE_TTL, Timestamp::now()) => Ok(cached),
            cached => match (
                self.read_state(device, Request::State(device.id)).await,
                cached,
            ) {
                (Ok(state), _) => Ok(state),
                (Err(AppError::Refused(Refusal::TooSoon { .. })), Some(recent)) => Ok(recent),
                (Err(error), _) => Err(error),
            },
        }
    }

    /// Sends a command and caches the state the server answers with. That state shows the new
    /// values before the unit has applied them.
    pub async fn send(
        &self,
        device: &Device,
        command: &SetAtaRequest,
    ) -> Result<AtaState, AppError> {
        let account = self.account()?;
        let answer = self
            .call(
                Request::Command(device.id),
                self.client.set_ata(&account.session, command),
            )
            .await?;
        check_device(device, &answer)?;
        cache::store(&self.paths.state_of(device.id), &answer, Timestamp::now())?;
        Ok(answer)
    }
}

fn check_device(device: &Device, state: &AtaState) -> Result<(), AppError> {
    let answered = state.view().device_id;
    if answered == device.id {
        Ok(())
    } else {
        Err(AppError::WrongDevice {
            asked: device.id,
            answered,
        })
    }
}
