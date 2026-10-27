//! The interactive TUI: `klima` without a command.
//!
//! [`model`] holds the state and decides what to do, [`view`] draws it. This module does the
//! I/O: terminal input, the clock, background requests through [`App`] (so the request budget
//! applies), and watching the cache, so a command run from another terminal shows up here.

mod keymap;
mod model;
mod view;
mod widgets;

use std::fs;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use melcloud::{AtaState, ChangeSet, Device, DeviceList};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Position;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::app::App;
use crate::cache::{self, Cached, DEVICES_TTL};
use crate::control;
use crate::error::AppError;
use crate::governor::{self, Refusal, Request};
use crate::output;
use crate::session;
use model::{Account, Effect, Failure, FailureKind, Model, Msg, Row, Start, Target};
use view::Hotspot;

/// How often the clock, the countdown and the watched files are updated.
const TICK: Duration = Duration::from_secs(1);
/// Redraw interval while something moves on screen.
const FRAME: Duration = Duration::from_millis(80);

/// What reaches the event loop: messages for the model, and mouse events still to be placed.
enum Incoming {
    Msg(Box<Msg>),
    Mouse(MouseEvent),
}
const HISTORY_SPAN: SignedDuration = SignedDuration::from_hours(24);

pub async fn run(app: App) -> Result<(), AppError> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err(AppError::Usage(
            "`klima` without a command opens the TUI, which needs a terminal; see `klima --help`"
                .into(),
        ));
    }
    let app = Arc::new(app);
    let (mut model, effects) = Model::start(start(&app)?);
    let mut terminal = ratatui::init();
    let result = match execute!(io::stdout(), EnableMouseCapture) {
        Ok(()) => {
            // Leave the terminal's mouse alone even after a panic.
            let restore = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let _ = execute!(io::stdout(), DisableMouseCapture);
                restore(info);
            }));
            let result = event_loop(&app, &mut terminal, &mut model, effects).await;
            let _ = execute!(io::stdout(), DisableMouseCapture);
            result
        }
        Err(e) => Err(AppError::io("terminal", e)),
    };
    ratatui::restore();
    result
}

/// What the TUI shows before the first request: everything cached on disk.
fn start(app: &App) -> Result<Start, AppError> {
    let now = Timestamp::now();
    let stored = session::load(&app.paths)?;
    let preferred_device = app.config.device.clone();
    let devices =
        cache::load::<DeviceList>(&app.paths.devices()).filter(|d| d.fresh(DEVICES_TTL, now));
    let device = devices.as_ref().and_then(|list| {
        let devices = list.value.devices();
        model::choose(devices, preferred_device.as_deref()).map(|index| devices[index].id)
    });
    let state = device.and_then(|id| cache::load::<AtaState>(&app.paths.state_of(id)));
    let history = device
        .map(|id| cache::room_history(&app.paths, id, now - HISTORY_SPAN))
        .unwrap_or_default();
    let email = app
        .config
        .email
        .clone()
        .or_else(|| stored.as_ref().map(|s| s.email.clone()));
    let account = stored.filter(|s| !s.expired(now)).map(|s| Account {
        email: s.email,
        expires_at: s.session.expires_at(),
    });
    Ok(Start {
        now,
        tz: TimeZone::system(),
        refresh_every: SignedDuration::from_mins(i64::from(app.config.refresh_minutes())),
        preferred_device,
        devices,
        state,
        history,
        requests_last_hour: governor::requests_last_hour(&app.paths, now),
        account,
        email,
    })
}

async fn event_loop(
    app: &Arc<App>,
    terminal: &mut DefaultTerminal,
    model: &mut Model,
    effects: Vec<Effect>,
) -> Result<(), AppError> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    read_input(tx.clone());
    for effect in effects {
        spawn(app, &tx, effect);
    }
    let mut watched = Watched::default();
    let mut observed = Instant::now();
    let mut hotspots: Vec<Hotspot> = Vec::new();
    while !model.quit {
        terminal
            .draw(|frame| hotspots = view::render(model, frame))
            .map_err(|e| AppError::io("terminal", e))?;
        let wait = if model.animating() { FRAME } else { TICK };
        let first = tokio::select! {
            Some(incoming) = rx.recv() => incoming,
            () = tokio::time::sleep(wait) => Incoming::Msg(Box::new(Msg::Tick(Timestamp::now()))),
        };
        let mut incoming = vec![first];
        while let Ok(more) = rx.try_recv() {
            incoming.push(more);
        }
        let mut messages: Vec<Msg> = incoming
            .into_iter()
            .filter_map(|incoming| match incoming {
                Incoming::Msg(msg) => Some(*msg),
                Incoming::Mouse(event) => place(event, &hotspots),
            })
            .collect();
        messages.push(Msg::Tick(Timestamp::now()));
        if observed.elapsed() >= TICK {
            observed = Instant::now();
            messages.extend(watched.observe(app, model));
        }
        for msg in messages {
            for effect in model.update(msg) {
                spawn(app, &tx, effect);
            }
        }
    }
    Ok(())
}

/// The message a click or the wheel stands for, by what is under the pointer.
fn place(event: MouseEvent, hotspots: &[Hotspot]) -> Option<Msg> {
    let pointer = Position::new(event.column, event.row);
    let target = hotspots
        .iter()
        .rev()
        .find(|spot| spot.area.contains(pointer))?
        .target;
    let temperature = target == Target::Row(Row::Temperature);
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => Some(Msg::Target(target)),
        MouseEventKind::ScrollUp if temperature => Some(Msg::Wheel(1)),
        MouseEventKind::ScrollDown if temperature => Some(Msg::Wheel(-1)),
        _ => None,
    }
}

/// Reads terminal events on their own thread, since reading blocks.
fn read_input(tx: UnboundedSender<Incoming>) {
    std::thread::spawn(move || {
        loop {
            let incoming = match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    Incoming::Msg(Box::new(Msg::Key(key)))
                }
                // A tick redraws at the new size.
                Ok(Event::Resize(..)) => Incoming::Msg(Box::new(Msg::Tick(Timestamp::now()))),
                // Moves and drags would only cost redraws.
                Ok(Event::Mouse(mouse))
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Down(_)
                            | MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                    ) =>
                {
                    Incoming::Mouse(mouse)
                }
                Ok(_) => continue,
                Err(_) => return,
            };
            if tx.send(incoming).is_err() {
                return;
            }
        }
    });
}

/// Runs background work through [`App`] and reports back with a message.
fn spawn(app: &Arc<App>, tx: &UnboundedSender<Incoming>, effect: Effect) {
    let app = Arc::clone(app);
    let tx = tx.clone();
    tokio::spawn(async move {
        // The receiver is gone only when the TUI has quit.
        let report = |msg| {
            let _ = tx.send(Incoming::Msg(Box::new(msg)));
        };
        match effect {
            Effect::LoadDevices => {
                report(Msg::Devices(
                    load_devices(&app).await.map_err(|e| failure(&e)),
                ));
            }
            Effect::Refresh { device, manual } => report(Msg::State {
                result: app
                    .read_state(&device, Request::State(device.id))
                    .await
                    .map_err(|e| failure(&e)),
                manual,
            }),
            Effect::Login { email, password } => report(Msg::LoggedIn(
                app.login(&email, &password)
                    .await
                    .map(|session| Account {
                        email,
                        expires_at: session.expires_at(),
                    })
                    .map_err(|e| failure(&e)),
            )),
            Effect::Send { device, changes } => send(&app, &device, changes, report).await,
        }
    });
}

/// Sends `changes` as one command and checks the unit applied it, the same way `klima set`
/// does: built on the freshest state the budget allows, without what the unit already has.
async fn send(app: &App, device: &Device, changes: ChangeSet, report: impl Fn(Msg)) {
    let sent: Result<_, AppError> = async {
        let base = app.base_state(device).await?;
        let changes = control::changes(changes, base.value.view());
        if changes.is_empty() {
            return Err(AppError::Usage(
                "nothing to send: the unit already has these settings".into(),
            ));
        }
        let command = changes
            .build(device, &base.value)
            .map_err(AppError::Invalid)?;
        let sent_at = Timestamp::now();
        let answer = app.send(device, &command).await?;
        Ok((changes, answer, sent_at))
    }
    .await;
    let (changes, answer, sent_at) = match sent {
        Ok(sent) => sent,
        Err(error) => return report(Msg::Sent(Err(failure(&error)))),
    };
    report(Msg::Sent(Ok(())));
    let result = control::confirm(app, device, &changes, &answer, sent_at, false)
        .await
        .map_err(|e| failure(&e));
    let state = cache::load(&app.paths.state_of(device.id));
    report(Msg::Confirmed { result, state });
}

async fn load_devices(
    app: &App,
) -> Result<(Cached<DeviceList>, Option<Cached<AtaState>>), AppError> {
    let list = app.devices(false).await?;
    let devices = list.value.devices();
    let state = model::choose(devices, app.config.device.as_deref())
        .and_then(|index| cache::load(&app.paths.state_of(devices[index].id)));
    Ok((list, state))
}

fn failure(error: &AppError) -> Failure {
    let kind = match error {
        AppError::NotLoggedIn | AppError::SessionExpired => FailureKind::NeedsLogin,
        AppError::Refused(Refusal::TooSoon { .. }) => FailureKind::TooSoon,
        AppError::Refused(_) | AppError::Throttled { .. } | AppError::LoginLocked { .. } => {
            FailureKind::Blocked
        }
        _ => FailureKind::Other,
    };
    Failure {
        kind,
        message: output::message(error),
    }
}

/// Files that other klima processes write: the budget ledger, the unit's cached state and the
/// history.
#[derive(Default)]
struct Watched {
    state: Option<SystemTime>,
    history: Option<SystemTime>,
}

impl Watched {
    fn observe(&mut self, app: &App, model: &Model) -> Vec<Msg> {
        let now = Timestamp::now();
        let mut messages = vec![Msg::Budget(governor::requests_last_hour(&app.paths, now))];
        let Some(device) = model.device() else {
            return messages;
        };
        let state_path = app.paths.state_of(device.id);
        if changed(&state_path, &mut self.state)
            && let Some(state) = cache::load(&state_path)
        {
            messages.push(Msg::External(state));
        }
        if changed(&app.paths.history(), &mut self.history) {
            let history = cache::room_history(&app.paths, device.id, now - HISTORY_SPAN);
            messages.push(Msg::History(history));
        }
        messages
    }
}

/// Whether `path` was modified since `seen`, which it then updates.
fn changed(path: &Path, seen: &mut Option<SystemTime>) -> bool {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok();
    let changed = modified.is_some() && modified != *seen;
    *seen = modified;
    changed
}
