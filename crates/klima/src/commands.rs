//! The CLI commands.

use std::io::{self, BufRead, IsTerminal, Write};

use jiff::Timestamp;
use melcloud::{AtaState, Device};

use crate::app::App;
use crate::cache::{self, Cached, STATE_TTL};
use crate::error::AppError;
use crate::governor::{self, Refusal, Request};
use crate::output::{self, Source, StatusReport};
use crate::session;

/// `klima login`: asks for the password (or reads it from stdin) and stores only the session.
pub async fn login(app: &App, email: Option<String>, password_stdin: bool) -> Result<(), AppError> {
    let previous = session::load(&app.paths)?.map(|account| account.email);
    let email = match email.or_else(|| app.config.email.clone()) {
        Some(email) => email,
        None if password_stdin => {
            return Err(AppError::Usage(
                "--password-stdin needs --email or `email` in config.toml".into(),
            ));
        }
        None => prompt_email(previous.as_deref())?,
    };
    let password = if password_stdin {
        read_stdin_line()?
    } else {
        prompt_password()?
    };
    if password.is_empty() {
        return Err(AppError::Usage("the password is empty".into()));
    }

    let session = app
        .call(Request::Login, app.client.login(&email, &password, true))
        .await?;
    let now = Timestamp::now();
    session::save(&app.paths, &email, &session, now)?;
    match session.expires_at() {
        Some(until) => output::success(format!(
            "Logged in as {email}. The session lasts until {} ({}).",
            output::local_time(until),
            output::duration(until.duration_since(now)),
        )),
        None => output::success(format!("Logged in as {email}.")),
    }
    Ok(())
}

fn prompt_email(previous: Option<&str>) -> Result<String, AppError> {
    if !io::stdin().is_terminal() {
        return Err(AppError::Usage(
            "no terminal to ask for the email; pass --email".into(),
        ));
    }
    match previous {
        Some(email) => eprint!("MELCloud email [{email}]: "),
        None => eprint!("MELCloud email: "),
    }
    io::stderr()
        .flush()
        .map_err(|e| AppError::io("stderr", e))?;
    let typed = read_stdin_line()?;
    let email = match typed.trim() {
        "" => previous.ok_or_else(|| AppError::Usage("no email given".into()))?,
        typed => typed,
    };
    Ok(email.to_owned())
}

fn prompt_password() -> Result<String, AppError> {
    rpassword::prompt_password("MELCloud password: ").map_err(|_| {
        AppError::Usage("no terminal to ask for the password; use --password-stdin".into())
    })
}

fn read_stdin_line() -> Result<String, AppError> {
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| AppError::io("stdin", e))?;
    Ok(line.trim_end_matches(['\n', '\r']).to_owned())
}

/// `klima logout`: forgets the session.
pub fn logout(app: &App) -> Result<(), AppError> {
    if session::delete(&app.paths)? {
        output::success("Logged out.");
    } else {
        output::note("Not logged in.");
    }
    Ok(())
}

/// `klima unblock`: lifts the local pause after HTTP 429.
pub fn unblock(app: &App) -> Result<(), AppError> {
    if governor::update(&app.paths, |ledger| ledger.unblock(Timestamp::now()))? {
        output::success(
            "Requests are allowed again. MELCloud may still throttle the account for a while.",
        );
    } else {
        output::note("Nothing was blocked.");
    }
    Ok(())
}

/// `klima devices`.
pub async fn devices(app: &App, fresh: bool, json: bool) -> Result<(), AppError> {
    let list = app.devices(fresh).await?;
    if json {
        output::devices_json(list.value.devices());
    } else {
        output::devices(list.value.devices(), list.fetched_at);
    }
    Ok(())
}

/// Picks the device by name or ID, the configured one, or the only one.
pub fn select<'a>(devices: &'a [Device], wanted: Option<&str>) -> Result<&'a Device, AppError> {
    let names = || {
        devices
            .iter()
            .map(|d| format!("\"{}\"", d.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let device = match (wanted, devices) {
        (Some(wanted), _) => devices
            .iter()
            .find(|d| d.id.to_string() == wanted || d.name.eq_ignore_ascii_case(wanted))
            .ok_or_else(|| {
                AppError::Usage(format!(
                    "no device \"{wanted}\"; the account has {}",
                    names()
                ))
            })?,
        (None, [only]) => only,
        (None, []) => return Err(AppError::Usage("the account has no devices".into())),
        (None, _) => {
            return Err(AppError::Usage(format!(
                "the account has several devices; pick one with --device or `device` in config.toml: {}",
                names()
            )));
        }
    };
    if device.capabilities.is_none() {
        return Err(AppError::Usage(format!(
            "\"{}\" is not an air conditioner",
            device.name
        )));
    }
    Ok(device)
}

/// `klima status`: the cached state while it is fresh, otherwise one read from the server. When
/// the server cannot be asked, the last known state is shown with the reason.
pub async fn status(
    app: &App,
    wanted: Option<&str>,
    fresh: bool,
    json: bool,
) -> Result<(), AppError> {
    let list = app.devices(false).await?;
    let wanted = wanted.or(app.config.device.as_deref());
    let device = select(list.value.devices(), wanted)?;
    let cached: Option<Cached<AtaState>> = cache::load(&app.paths.state_of(device.id));

    let (state, source, problem) = match cached {
        Some(cached) if !fresh && cached.fresh(STATE_TTL, Timestamp::now()) => {
            (cached, Source::Cache, None)
        }
        cached => match (
            app.read_state(device, Request::State(device.id)).await,
            cached,
        ) {
            (Ok(state), _) => (state, Source::Server, None),
            (Err(error), Some(cached)) => (cached, Source::Cache, Some(error)),
            (Err(error), None) => return Err(error),
        },
    };
    // A read refused only because the last one was moments ago leaves the data current.
    let problem = problem.filter(|e| !matches!(e, AppError::Refused(Refusal::TooSoon { .. })));
    let report = StatusReport {
        device,
        state: &state.value,
        fetched_at: state.fetched_at,
        source,
        stale_because: problem.as_ref().map(ToString::to_string),
    };
    if json {
        output::status_json(&report);
    } else {
        let caps = device
            .capabilities
            .as_ref()
            .expect("select() only returns air conditioners");
        output::status(&report, caps);
    }
    problem.map_or(Ok(()), Err)
}
