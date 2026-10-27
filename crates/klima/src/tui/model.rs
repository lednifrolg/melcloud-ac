//! What the TUI knows and how messages change it. Pure: I/O happens in [`super`], so this is
//! tested directly.

use std::collections::VecDeque;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use melcloud::{
    AtaState, AtaView, Capabilities, ChangeSet, Device, DeviceList, FanSpeed, OperationMode,
    VaneHorizontal, VaneVertical,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::keymap::{self, Action};
use crate::cache::{Cached, STATE_TTL, Sample};
use crate::control;

/// Panes as tabs, for narrow windows.
pub const TABS: [&str; 4] = ["control", "info", "history", "activity"];
const ACTIVITY_LIMIT: usize = 100;
/// After a failure that is not about limits, try again this soon.
const RETRY_AFTER_FAILURE: SignedDuration = SignedDuration::from_mins(2);
/// Changes made within this long of each other go out as one command.
pub const MERGE_WINDOW: SignedDuration = SignedDuration::from_secs(3);
/// How long the result of a command stays in the control pane.
pub const NOTICE_SHOWN: SignedDuration = SignedDuration::from_secs(5);
/// How long the big digits light up when the target changes.
pub const FLASH: SignedDuration = SignedDuration::from_millis(300);

/// A setting row of the control pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Temperature,
    Mode,
    Fan,
    VaneVertical,
    VaneHorizontal,
}

/// The rows a unit with `caps` has, top to bottom.
pub fn rows(caps: &Capabilities) -> Vec<Row> {
    let mut rows = vec![Row::Temperature, Row::Mode];
    if caps.fan_levels > 0 {
        rows.push(Row::Fan);
    }
    if caps.vane_vertical {
        rows.push(Row::VaneVertical);
    }
    if caps.vane_horizontal {
        rows.push(Row::VaneHorizontal);
    }
    rows
}

/// What a mouse click points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Row(Row),
    Mode(OperationMode),
    Fan(FanSpeed),
}

/// A change asked for with a key or the mouse, before it is fitted to the unit.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Edit {
    /// Steps of the unit's temperature increment.
    Temperature(i32),
    Mode(OperationMode),
    NextMode(i32),
    Fan(FanSpeed),
    NextFan(i32),
    NextVertical(i32),
    NextHorizontal(i32),
    SwingVertical,
    SwingHorizontal,
    Power,
}

/// Changes waiting for the merge window to close.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pending {
    pub changes: ChangeSet,
    pub deadline: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Sending,
    /// Sent; waiting for the adapter to pick the command up.
    Confirming,
}

/// A command on its way to the unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    pub changes: ChangeSet,
    pub phase: Phase,
    pub since: Timestamp,
}

/// What is being fetched in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Devices,
    State,
    Login,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Good,
    Warn,
    Bad,
}

/// One line of the activity pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub at: Timestamp,
    pub tone: Tone,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    NeedsLogin,
    /// The unit was read moments ago.
    TooSoon,
    /// Throttling, the hourly budget or a login lockout.
    Blocked,
    Other,
}

/// Why a background request failed, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    pub on_password: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    Login(LoginForm),
}

/// The logged-in account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub email: String,
    pub expires_at: Option<Timestamp>,
}

pub enum Msg {
    Key(KeyEvent),
    Tick(Timestamp),
    /// The device list, and the cached state of the chosen device.
    Devices(Result<(Cached<DeviceList>, Option<Cached<AtaState>>), Failure>),
    State {
        result: Result<Cached<AtaState>, Failure>,
        manual: bool,
    },
    LoggedIn(Result<Account, Failure>),
    /// Whether the server took the command.
    Sent(Result<(), Failure>),
    /// How the check of the command ended, and the state stored meanwhile.
    Confirmed {
        result: Result<Option<SignedDuration>, Failure>,
        state: Option<Cached<AtaState>>,
    },
    Target(Target),
    /// Mouse wheel over the temperature: steps up (positive) or down.
    Wheel(i32),
    /// Another klima process stored a newer state.
    External(Cached<AtaState>),
    Budget(usize),
    History(Vec<Sample>),
}

/// Background work the TUI asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    LoadDevices,
    Refresh {
        device: Box<Device>,
        manual: bool,
    },
    Login {
        email: String,
        password: String,
    },
    Send {
        device: Box<Device>,
        changes: ChangeSet,
    },
}

/// Everything the TUI starts with, read from disk.
pub struct Start {
    pub now: Timestamp,
    pub tz: TimeZone,
    pub refresh_every: SignedDuration,
    pub preferred_device: Option<String>,
    pub devices: Option<Cached<DeviceList>>,
    pub state: Option<Cached<AtaState>>,
    pub history: Vec<Sample>,
    pub requests_last_hour: usize,
    pub account: Option<Account>,
    /// Offered in the login form.
    pub email: Option<String>,
    /// Spinning fan, swinging vanes and flashing digits.
    pub animations: bool,
}

/// How far quitting has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quit {
    No,
    /// Asked for while changes wait to be sent; what to do with them is asked first.
    Asking,
    /// Once the server has taken the command on its way.
    AfterSend,
    Now,
}

pub struct Model {
    pub now: Timestamp,
    pub tz: TimeZone,
    pub refresh_every: SignedDuration,
    pub devices: Option<Cached<DeviceList>>,
    pub device: Option<usize>,
    pub state: Option<Cached<AtaState>>,
    pub history: Vec<Sample>,
    pub busy: Option<Busy>,
    pub next_refresh: Option<Timestamp>,
    pub requests_last_hour: usize,
    pub account: Option<Account>,
    pub activity: VecDeque<Entry>,
    pub overlay: Overlay,
    pub tab: usize,
    pub row: Row,
    pub pending: Option<Pending>,
    pub flight: Option<Flight>,
    /// The latest result or hint for the control pane.
    pub notice: Option<Entry>,
    pub quit: Quit,
    pub animations: bool,
    /// Until when the big digits light up after the target changed.
    pub flash_until: Option<Timestamp>,
    preferred_device: Option<String>,
    email: Option<String>,
}

/// What a user would notice changed between two states of the unit.
fn differences(before: &AtaView, after: &AtaView) -> Vec<String> {
    let mut changes = Vec::new();
    if before.power != after.power {
        changes.push(format!("power {}", if after.power { "on" } else { "off" }));
    }
    if before.mode != after.mode {
        changes.push(format!("mode {}", after.mode));
    }
    if (before.target_temperature - after.target_temperature).abs() > 1e-6 {
        changes.push(format!("{:.1} °C", after.target_temperature));
    }
    if before.fan_speed != after.fan_speed {
        changes.push(format!("fan {}", after.fan_speed));
    }
    if before.vane_vertical != after.vane_vertical
        || before.vane_horizontal != after.vane_horizontal
    {
        changes.push(format!(
            "vanes {} / {}",
            after.vane_vertical, after.vane_horizontal
        ));
    }
    changes
}

/// Puts `changes` into `view`, as the unit will have them.
fn apply(view: &mut AtaView, changes: ChangeSet) {
    if let Some(power) = changes.power {
        view.power = power;
    }
    if let Some(mode) = changes.mode {
        view.mode = mode;
    }
    if let Some(target) = changes.target_temperature {
        view.target_temperature = target;
    }
    if let Some(fan) = changes.fan_speed {
        view.fan_speed = fan;
    }
    if let Some(vane) = changes.vane_vertical {
        view.vane_vertical = vane;
    }
    if let Some(vane) = changes.vane_horizontal {
        view.vane_horizontal = vane;
    }
}

/// The new values of `changes`, e.g. "power on, 22.5 °C, fan auto".
pub fn summary(changes: ChangeSet) -> String {
    let mut parts = Vec::new();
    if let Some(power) = changes.power {
        parts.push(format!("power {}", if power { "on" } else { "off" }));
    }
    if let Some(mode) = changes.mode {
        parts.push(format!("mode {mode}"));
    }
    if let Some(target) = changes.target_temperature {
        parts.push(format!("{target:.1} °C"));
    }
    if let Some(fan) = changes.fan_speed {
        parts.push(format!("fan {fan}"));
    }
    if let Some(vane) = changes.vane_vertical {
        parts.push(format!("vane ↕ {vane}"));
    }
    if let Some(vane) = changes.vane_horizontal {
        parts.push(format!("vane ↔ {vane}"));
    }
    parts.join(", ")
}

/// The option `step` places after `current` in `options`, wrapping around; the first one when
/// `current` is not among them.
fn cycle<T: Copy + PartialEq>(options: &[T], current: T, step: i32) -> Option<T> {
    let count = i32::try_from(options.len()).ok().filter(|&n| n > 0)?;
    let Some(index) = options.iter().position(|&o| o == current) else {
        return options.first().copied();
    };
    let next = (i32::try_from(index).ok()? + step).rem_euclid(count);
    options.get(usize::try_from(next).ok()?).copied()
}

fn fan_speeds(caps: &Capabilities) -> Vec<FanSpeed> {
    let auto = caps.fan_auto.then_some(FanSpeed::Auto);
    auto.into_iter()
        .chain((1..=caps.fan_levels).map(FanSpeed::Level))
        .collect()
}

fn vertical_positions() -> Vec<VaneVertical> {
    let positions = (1..=5).map(VaneVertical::Position);
    std::iter::once(VaneVertical::Auto)
        .chain(positions)
        .chain([VaneVertical::Swing])
        .collect()
}

fn horizontal_positions(caps: &Capabilities) -> Vec<VaneHorizontal> {
    let positions = (1..=5).map(VaneHorizontal::Position);
    let split = caps.wide_vane.then_some(VaneHorizontal::Split);
    std::iter::once(VaneHorizontal::Auto)
        .chain(positions)
        .chain(split)
        .chain([VaneHorizontal::Swing])
        .collect()
}

/// The device to show: the preferred one by name or ID, else the first air conditioner.
pub fn choose(devices: &[Device], preferred: Option<&str>) -> Option<usize> {
    let air_conditioners = || {
        devices
            .iter()
            .enumerate()
            .filter(|(_, d)| d.capabilities.is_some())
    };
    preferred
        .and_then(|wanted| {
            air_conditioners()
                .find(|(_, d)| d.id.to_string() == wanted || d.name.eq_ignore_ascii_case(wanted))
        })
        .or_else(|| air_conditioners().next())
        .map(|(index, _)| index)
}

impl Model {
    pub fn start(start: Start) -> (Self, Vec<Effect>) {
        let device = start
            .devices
            .as_ref()
            .and_then(|list| choose(list.value.devices(), start.preferred_device.as_deref()));
        let mut model = Self {
            now: start.now,
            tz: start.tz,
            refresh_every: start.refresh_every,
            devices: start.devices,
            device,
            state: None,
            history: start.history,
            busy: None,
            next_refresh: None,
            requests_last_hour: start.requests_last_hour,
            account: start.account,
            activity: VecDeque::new(),
            overlay: Overlay::None,
            tab: 0,
            row: Row::Temperature,
            pending: None,
            flight: None,
            notice: None,
            quit: Quit::No,
            animations: start.animations,
            flash_until: None,
            preferred_device: start.preferred_device,
            email: start.email,
        };
        if let Some(state) = start.state {
            model.adopt(state);
        }
        if model.account.is_none() {
            model.log(Tone::Warn, "not logged in");
            model.open_login();
        }
        let effects = model.due();
        (model, effects)
    }

    pub fn device(&self) -> Option<&Device> {
        Some(&self.devices.as_ref()?.value.devices()[self.device?])
    }

    /// The unit as the control pane shows it: the last known state with the changes on their
    /// way applied.
    pub fn shown(&self) -> Option<AtaView> {
        let mut view = self.state.as_ref()?.value.view().clone();
        let on_the_way = [
            self.flight.map(|f| f.changes),
            self.pending.map(|p| p.changes),
        ];
        for changes in on_the_way.into_iter().flatten() {
            apply(&mut view, changes);
        }
        Some(view)
    }

    /// Whether something changes quickly on screen: countdowns, spinners, a flash.
    pub fn animating(&self) -> bool {
        let recent_notice = self
            .notice
            .as_ref()
            .is_some_and(|n| n.at + NOTICE_SHOWN > self.now);
        let flashing = self.flash_until.is_some_and(|until| until > self.now);
        self.pending.is_some() || self.flight.is_some() || recent_notice || flashing
    }

    /// Whether the running unit is drawn in motion: a spinning fan, swinging vanes.
    pub fn moving(&self) -> bool {
        self.animations && self.shown().is_some_and(|view| view.power)
    }

    /// Handles `msg`, and lights the big digits up when it changed the target shown, whatever
    /// the cause: a key, a discard, a read, another klima.
    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        let before = self.shown().map(|view| view.target_temperature);
        let effects = self.handle(msg);
        let after = self.shown().map(|view| view.target_temperature);
        let changed = matches!((before, after), (Some(a), Some(b)) if (a - b).abs() > 1e-6);
        if changed && self.animations {
            self.flash_until = Some(self.now + FLASH);
        }
        effects
    }

    fn handle(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Key(key) => return self.key(key),
            Msg::Tick(now) => self.now = now,
            Msg::Devices(Ok((devices, state))) => {
                self.busy = None;
                self.device = choose(devices.value.devices(), self.preferred_device.as_deref());
                self.devices = Some(devices);
                if self.device.is_none() {
                    self.log(Tone::Warn, "no air conditioner on this account");
                }
                if let Some(state) = state {
                    self.adopt(state);
                }
            }
            Msg::State {
                result: Ok(state),
                manual,
            } => {
                self.busy = None;
                self.adopt(state);
                self.log(
                    Tone::Good,
                    if manual { "refreshed" } else { "auto refresh" },
                );
            }
            Msg::Devices(Err(failure))
            | Msg::State {
                result: Err(failure),
                ..
            } => {
                self.busy = None;
                self.failed(failure);
            }
            Msg::LoggedIn(Ok(account)) => {
                self.busy = None;
                self.overlay = Overlay::None;
                self.log(Tone::Good, format!("logged in as {}", account.email));
                self.account = Some(account);
                self.next_refresh = self
                    .state
                    .as_ref()
                    .map(|s| s.fetched_at + self.refresh_every);
            }
            Msg::LoggedIn(Err(failure)) => {
                self.busy = None;
                if let Overlay::Login(form) = &mut self.overlay {
                    form.error = Some(failure.message);
                    form.password.clear();
                }
            }
            Msg::External(state) => {
                let newer = self
                    .state
                    .as_ref()
                    .is_none_or(|s| state.fetched_at > s.fetched_at);
                // While a command is on its way, the stored states are its own.
                if newer && self.busy != Some(Busy::State) && self.flight.is_none() {
                    let changes = self
                        .state
                        .as_ref()
                        .map(|old| differences(old.value.view(), state.value.view()));
                    self.adopt(state);
                    if let Some(changes) = changes.filter(|c| !c.is_empty()) {
                        self.log(Tone::Info, format!("another klima: {}", changes.join(", ")));
                    }
                }
            }
            Msg::Sent(result) => self.sent(result),
            Msg::Confirmed { result, state } => self.confirmed(result, state),
            Msg::Target(target) => self.point_at(target),
            Msg::Wheel(steps) => {
                self.row = Row::Temperature;
                self.edit(Edit::Temperature(steps));
            }
            Msg::Budget(requests) => self.requests_last_hour = requests,
            Msg::History(history) => self.history = history,
        }
        self.due()
    }

    /// The server took the command, or refused it. The answer already carries the new values;
    /// the control pane shows them from the flight, and the confirmed state replaces the last
    /// known one when the check ends.
    fn sent(&mut self, result: Result<(), Failure>) {
        match result {
            Ok(()) => {
                if let Some(flight) = &mut self.flight {
                    flight.phase = Phase::Confirming;
                }
            }
            Err(failure) => {
                self.flight = None;
                self.notify(Tone::Bad, failure.message.clone());
                self.failed(failure);
            }
        }
        if self.quit == Quit::AfterSend {
            self.quit = Quit::Now;
        }
    }

    fn confirmed(
        &mut self,
        result: Result<Option<SignedDuration>, Failure>,
        state: Option<Cached<AtaState>>,
    ) {
        let flight = self.flight.take();
        if let Some(state) = state {
            self.adopt(state);
        }
        match result {
            Ok(delay) => {
                let text = delay.map_or_else(
                    || "applied".to_owned(),
                    |d| {
                        format!(
                            "applied · the adapter took it {:.1} s after sending",
                            d.as_secs_f64()
                        )
                    },
                );
                self.notify(Tone::Good, text);
                let what = flight.map(|f| summary(f.changes)).unwrap_or_default();
                self.log(Tone::Good, format!("applied: {what}"));
            }
            Err(failure) => {
                self.notify(Tone::Bad, failure.message.clone());
                self.log(Tone::Bad, failure.message);
            }
        }
    }

    fn point_at(&mut self, target: Target) {
        match target {
            Target::Row(row) => self.row = row,
            Target::Mode(mode) => {
                self.row = Row::Mode;
                self.edit(Edit::Mode(mode));
            }
            Target::Fan(fan) => {
                self.row = Row::Fan;
                self.edit(Edit::Fan(fan));
            }
        }
    }

    /// Shows `state` and schedules the next automatic refresh after it.
    fn adopt(&mut self, state: Cached<AtaState>) {
        self.next_refresh = Some(state.fetched_at + self.refresh_every);
        self.state = Some(state);
    }

    /// Starts the background work that is due: the device list first, then a state read once
    /// the cached state is too old.
    fn due(&mut self) -> Vec<Effect> {
        // While quitting is asked about, the changes wait for the answer.
        let closed =
            self.pending.is_some_and(|p| p.deadline <= self.now) && self.quit != Quit::Asking;
        if closed && self.flight.is_none() {
            return self.flush();
        }
        if self.pending.is_some() || self.flight.is_some() {
            return Vec::new();
        }
        let logging_in = matches!(self.overlay, Overlay::Login(_));
        if self.busy.is_some() || self.account.is_none() || logging_in {
            return Vec::new();
        }
        if self.devices.is_none() {
            self.busy = Some(Busy::Devices);
            return vec![Effect::LoadDevices];
        }
        let stale = self
            .state
            .as_ref()
            .is_none_or(|s| !s.fresh(STATE_TTL, self.now));
        let scheduled = self.next_refresh.is_none_or(|at| at <= self.now);
        if stale && scheduled {
            return self.refresh(false);
        }
        Vec::new()
    }

    fn refresh(&mut self, manual: bool) -> Vec<Effect> {
        let Some(device) = self.device().cloned() else {
            return Vec::new();
        };
        if self.busy.is_some() {
            return Vec::new();
        }
        self.busy = Some(Busy::State);
        vec![Effect::Refresh {
            device: Box::new(device),
            manual,
        }]
    }

    fn failed(&mut self, failure: Failure) {
        match failure.kind {
            FailureKind::NeedsLogin => {
                self.account = None;
                self.open_login();
            }
            FailureKind::TooSoon => {}
            FailureKind::Blocked => self.next_refresh = Some(self.now + self.refresh_every),
            FailureKind::Other => self.next_refresh = Some(self.now + RETRY_AFTER_FAILURE),
        }
        let tone = match failure.kind {
            FailureKind::TooSoon => Tone::Info,
            FailureKind::NeedsLogin => Tone::Warn,
            FailureKind::Blocked | FailureKind::Other => Tone::Bad,
        };
        self.log(tone, failure.message);
    }

    fn open_login(&mut self) {
        let email = self
            .account
            .as_ref()
            .map(|a| a.email.clone())
            .or_else(|| self.email.clone())
            .unwrap_or_default();
        self.overlay = Overlay::Login(LoginForm {
            on_password: !email.is_empty(),
            email,
            ..LoginForm::default()
        });
    }

    fn log(&mut self, tone: Tone, text: impl Into<String>) {
        self.activity.push_front(Entry {
            at: self.now,
            tone,
            text: text.into(),
        });
        self.activity.truncate(ACTIVITY_LIMIT);
    }

    fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if ctrl_c {
            self.quit = Quit::Now;
            return Vec::new();
        }
        if let Overlay::Login(form) = &self.overlay {
            let form = form.clone();
            return self.login_key(key, form);
        }
        if self.overlay == Overlay::Help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?' | 'q')) {
                self.overlay = Overlay::None;
            }
            return Vec::new();
        }
        if self.quit == Quit::Asking {
            return self.quitting_key(key);
        }
        let on_the_way = self.pending.is_some() || self.flight.is_some();
        match keymap::action(key) {
            Some(Action::Up) => self.select(-1),
            Some(Action::Down) => self.select(1),
            Some(Action::Decrease) => self.change_row(-1),
            Some(Action::Increase) => self.change_row(1),
            Some(Action::Warmer) => self.edit(Edit::Temperature(1)),
            Some(Action::Cooler) => self.edit(Edit::Temperature(-1)),
            Some(Action::NextMode) => self.edit(Edit::NextMode(1)),
            Some(Action::NextFan) => self.edit(Edit::NextFan(1)),
            Some(Action::SwingVertical) => self.edit(Edit::SwingVertical),
            Some(Action::SwingHorizontal) => self.edit(Edit::SwingHorizontal),
            Some(Action::Power) => self.edit(Edit::Power),
            Some(Action::Send) => return self.flush(),
            Some(Action::Discard) => self.discard(),
            Some(Action::Refresh) if self.account.is_none() => self.open_login(),
            Some(Action::Refresh) if on_the_way => {}
            Some(Action::Refresh) => return self.refresh(true),
            Some(Action::NextTab) => self.tab = (self.tab + 1) % TABS.len(),
            Some(Action::Tab(index)) if index < TABS.len() => self.tab = index,
            Some(Action::Help) => self.overlay = Overlay::Help,
            Some(Action::Quit) if self.pending.is_some() => self.quit = Quit::Asking,
            Some(Action::Quit) if self.flight.is_some_and(|f| f.phase == Phase::Sending) => {
                self.quit = Quit::AfterSend;
            }
            Some(Action::Quit) => self.quit = Quit::Now,
            Some(Action::Tab(_)) | None => {}
        }
        Vec::new()
    }

    /// Keys while asking what to do with waiting changes before quitting.
    fn quitting_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Enter => {
                self.quit = Quit::AfterSend;
                return self.flush();
            }
            KeyCode::Char('u') => {
                self.pending = None;
                self.quit = Quit::Now;
            }
            KeyCode::Esc => self.quit = Quit::No,
            _ => {}
        }
        Vec::new()
    }

    fn select(&mut self, step: i32) {
        let Some(caps) = self.device().and_then(|d| d.capabilities.as_ref()) else {
            return;
        };
        let rows = rows(caps);
        let current = rows.iter().position(|&r| r == self.row).unwrap_or(0);
        let next = if step < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(rows.len() - 1)
        };
        self.row = rows[next];
    }

    fn change_row(&mut self, step: i32) {
        self.edit(match self.row {
            Row::Temperature => Edit::Temperature(step),
            Row::Mode => Edit::NextMode(step),
            Row::Fan => Edit::NextFan(step),
            Row::VaneVertical => Edit::NextVertical(step),
            Row::VaneHorizontal => Edit::NextHorizontal(step),
        });
    }

    /// Fits `edit` to what the unit supports and the account may change, and adds it to the
    /// changes waiting to be sent.
    fn edit(&mut self, edit: Edit) {
        let (Some(device), Some(shown)) = (self.device().cloned(), self.shown()) else {
            return;
        };
        let Some(caps) = device.capabilities.as_ref() else {
            return;
        };
        if self.account.is_none() {
            self.open_login();
            return;
        }
        let allowed = device.permissions;
        let refused = |what: &str| format!("this account may not change the {what}");
        let mut change = ChangeSet::default();
        match edit {
            Edit::Temperature(steps) => {
                let range = caps.temperature_range(shown.mode);
                let wanted = shown.target_temperature + f64::from(steps) * caps.temperature_step;
                let target = wanted.clamp(range.min, range.max);
                if (target - shown.target_temperature).abs() < 1e-6 {
                    let limit = format!("{}-{} °C in {} mode", range.min, range.max, shown.mode);
                    return self.notify(Tone::Info, limit);
                }
                change.target_temperature = Some(target);
            }
            Edit::Mode(_) | Edit::NextMode(_) if !allowed.can_set_operation_mode => {
                return self.notify(Tone::Warn, refused("mode"));
            }
            Edit::Mode(_) | Edit::NextMode(_) => {
                let mode = match edit {
                    Edit::NextMode(step) => cycle(&caps.modes, shown.mode, step),
                    Edit::Mode(mode) => Some(mode),
                    _ => None,
                };
                let Some(mode) = mode.filter(|m| caps.supports_mode(*m)) else {
                    return;
                };
                change.mode = Some(mode);
                let range = caps.temperature_range(mode);
                if !range.contains(shown.target_temperature) {
                    change.target_temperature =
                        Some(shown.target_temperature.clamp(range.min, range.max));
                }
            }
            Edit::Fan(_) | Edit::NextFan(_) if !allowed.can_set_fan_speed => {
                return self.notify(Tone::Warn, refused("fan speed"));
            }
            Edit::Fan(_) | Edit::NextFan(_) => {
                let speeds = fan_speeds(caps);
                change.fan_speed = match edit {
                    Edit::NextFan(step) => cycle(&speeds, shown.fan_speed, step),
                    Edit::Fan(fan) => speeds.contains(&fan).then_some(fan),
                    _ => None,
                };
            }
            Edit::NextVertical(_)
            | Edit::NextHorizontal(_)
            | Edit::SwingVertical
            | Edit::SwingHorizontal
                if !allowed.can_set_vane_direction =>
            {
                return self.notify(Tone::Warn, refused("vanes"));
            }
            Edit::NextVertical(step) => {
                change.vane_vertical = cycle(&vertical_positions(), shown.vane_vertical, step);
            }
            Edit::NextHorizontal(step) => {
                change.vane_horizontal =
                    cycle(&horizontal_positions(caps), shown.vane_horizontal, step);
            }
            Edit::SwingVertical if caps.vane_vertical => {
                change.vane_vertical = Some(if shown.vane_vertical == VaneVertical::Swing {
                    VaneVertical::Auto
                } else {
                    VaneVertical::Swing
                });
            }
            Edit::SwingHorizontal if caps.vane_horizontal => {
                change.vane_horizontal = Some(if shown.vane_horizontal == VaneHorizontal::Swing {
                    VaneHorizontal::Auto
                } else {
                    VaneHorizontal::Swing
                });
            }
            Edit::SwingVertical | Edit::SwingHorizontal => return,
            Edit::Power if !allowed.can_set_power => {
                return self.notify(Tone::Warn, refused("power"));
            }
            Edit::Power => change.power = Some(!shown.power),
        }
        self.queue(change);
    }

    /// Adds `change` to the waiting changes, leaves out what the unit already has, and restarts
    /// the merge window.
    fn queue(&mut self, change: ChangeSet) {
        let Some(mut base) = self.state.as_ref().map(|s| s.value.view().clone()) else {
            return;
        };
        if let Some(flight) = self.flight {
            apply(&mut base, flight.changes);
        }
        let mut changes = self.pending.map(|p| p.changes).unwrap_or_default();
        changes.merge(change);
        let changes = control::changes(changes, &base);
        self.notice = None;
        self.pending = (!changes.is_empty()).then_some(Pending {
            changes,
            deadline: self.now + MERGE_WINDOW,
        });
    }

    /// Sends the waiting changes now, unless a command is still on its way.
    fn flush(&mut self) -> Vec<Effect> {
        if self.flight.is_some() {
            return Vec::new();
        }
        let (Some(pending), Some(device)) = (self.pending, self.device().cloned()) else {
            return Vec::new();
        };
        self.pending = None;
        self.flight = Some(Flight {
            changes: pending.changes,
            phase: Phase::Sending,
            since: self.now,
        });
        vec![Effect::Send {
            device: Box::new(device),
            changes: pending.changes,
        }]
    }

    fn discard(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.log(
                Tone::Info,
                format!("discarded: {}", summary(pending.changes)),
            );
        }
    }

    fn notify(&mut self, tone: Tone, text: impl Into<String>) {
        self.notice = Some(Entry {
            at: self.now,
            tone,
            text: text.into(),
        });
    }

    fn login_key(&mut self, key: KeyEvent, mut form: LoginForm) -> Vec<Effect> {
        if self.busy == Some(Busy::Login) {
            return Vec::new();
        }
        let field = if form.on_password {
            &mut form.password
        } else {
            &mut form.email
        };
        match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                return Vec::new();
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up => {
                form.on_password = !form.on_password;
            }
            KeyCode::Backspace => {
                field.pop();
            }
            KeyCode::Char(c) => field.push(c),
            KeyCode::Enter if form.email.trim().is_empty() => form.on_password = false,
            KeyCode::Enter if form.password.is_empty() => form.on_password = true,
            KeyCode::Enter => {
                form.error = None;
                let effect = Effect::Login {
                    email: form.email.trim().to_owned(),
                    password: form.password.clone(),
                };
                self.overlay = Overlay::Login(form);
                self.busy = Some(Busy::Login);
                return vec![effect];
            }
            _ => {}
        }
        self.overlay = Overlay::Login(form);
        Vec::new()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const LIST_DEVICES: &str = include_str!("../../../melcloud/tests/data/list-devices.json");
    const DEVICE_GET: &str = include_str!("../../../melcloud/tests/data/device-get.json");

    pub fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    pub fn now() -> Timestamp {
        at("2026-01-15T10:05:30Z")
    }

    pub fn devices() -> Cached<DeviceList> {
        Cached {
            fetched_at: now() - SignedDuration::from_hours(1),
            value: serde_json::from_str(LIST_DEVICES).unwrap(),
        }
    }

    pub fn state(age: SignedDuration) -> Cached<AtaState> {
        Cached {
            fetched_at: now() - age,
            value: serde_json::from_str(DEVICE_GET).unwrap(),
        }
    }

    pub fn start() -> Start {
        Start {
            now: now(),
            tz: TimeZone::UTC,
            refresh_every: SignedDuration::from_mins(10),
            preferred_device: None,
            devices: Some(devices()),
            state: Some(state(SignedDuration::from_mins(2))),
            history: Vec::new(),
            requests_last_hour: 4,
            account: Some(Account {
                email: "user@example.com".into(),
                expires_at: Some(now() + SignedDuration::from_hours(24 * 364)),
            }),
            email: None,
            animations: true,
        }
    }

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::from(code))
    }

    #[test]
    fn fresh_cache_waits_for_the_next_scheduled_refresh() {
        let (mut model, effects) = Model::start(start());
        assert_eq!(effects, []);
        assert_eq!(
            model.next_refresh,
            Some(now() + SignedDuration::from_mins(8))
        );
        assert_eq!(
            model.update(Msg::Tick(now() + SignedDuration::from_mins(7))),
            []
        );
        let due = model.update(Msg::Tick(now() + SignedDuration::from_mins(8)));
        assert!(
            matches!(due.as_slice(), [Effect::Refresh { manual: false, .. }]),
            "{due:?}"
        );
        assert_eq!(model.busy, Some(Busy::State));
        assert_eq!(
            model.update(Msg::Tick(now() + SignedDuration::from_mins(9))),
            []
        );
    }

    #[test]
    fn stale_or_missing_state_is_read_at_once() {
        let stale = Start {
            state: Some(state(SignedDuration::from_mins(30))),
            ..start()
        };
        assert!(matches!(
            Model::start(stale).1.as_slice(),
            [Effect::Refresh { .. }]
        ));
        let missing = Start {
            devices: None,
            state: None,
            ..start()
        };
        assert_eq!(Model::start(missing).1, [Effect::LoadDevices]);
    }

    #[test]
    fn without_a_session_the_login_form_opens_and_nothing_is_fetched() {
        let (model, effects) = Model::start(Start {
            account: None,
            email: Some("user@example.com".into()),
            ..start()
        });
        assert_eq!(effects, []);
        let Overlay::Login(form) = &model.overlay else {
            panic!("no login form")
        };
        assert_eq!(form.email, "user@example.com");
        assert!(form.on_password);
    }

    #[test]
    fn login_form_submits_once_and_reads_a_stale_unit_after_success() {
        let (mut model, _) = Model::start(Start {
            account: None,
            state: Some(state(SignedDuration::from_mins(30))),
            ..start()
        });
        for c in "a@b.c".chars() {
            model.update(key(KeyCode::Char(c)));
        }
        model.update(key(KeyCode::Tab));
        for c in "pw".chars() {
            model.update(key(KeyCode::Char(c)));
        }
        let submitted = model.update(key(KeyCode::Enter));
        assert_eq!(
            submitted,
            [Effect::Login {
                email: "a@b.c".into(),
                password: "pw".into()
            }]
        );
        assert_eq!(model.update(key(KeyCode::Enter)), [], "submitted twice");

        let account = Account {
            email: "a@b.c".into(),
            expires_at: None,
        };
        let after = model.update(Msg::LoggedIn(Ok(account)));
        assert_eq!(model.overlay, Overlay::None);
        assert!(
            matches!(after.as_slice(), [Effect::Refresh { .. }]),
            "{after:?}"
        );
    }

    #[test]
    fn failed_login_keeps_the_form_with_the_reason() {
        let (mut model, _) = Model::start(Start {
            account: None,
            email: Some("a@b.c".into()),
            ..start()
        });
        model.update(key(KeyCode::Char('x')));
        model.update(key(KeyCode::Enter));
        model.update(Msg::LoggedIn(Err(Failure {
            kind: FailureKind::Other,
            message: "login failed: wrong email or password".into(),
        })));
        let Overlay::Login(form) = &model.overlay else {
            panic!("form closed")
        };
        assert_eq!(
            form.error.as_deref(),
            Some("login failed: wrong email or password")
        );
        assert_eq!(form.password, "");
        assert_eq!(model.busy, None);
    }

    #[test]
    fn manual_refresh_is_one_request_at_a_time() {
        let (mut model, _) = Model::start(start());
        let first = model.update(key(KeyCode::Char('r')));
        assert!(matches!(
            first.as_slice(),
            [Effect::Refresh { manual: true, .. }]
        ));
        assert_eq!(model.update(key(KeyCode::Char('r'))), []);
        model.update(Msg::State {
            result: Ok(state(SignedDuration::ZERO)),
            manual: true,
        });
        assert_eq!(model.activity[0].text, "refreshed");
        assert_eq!(
            model.next_refresh,
            Some(now() + SignedDuration::from_mins(10))
        );
    }

    #[test]
    fn failures_are_logged_and_set_the_next_try() {
        let (mut model, _) = Model::start(start());
        model.update(key(KeyCode::Char('r')));
        model.update(Msg::State {
            result: Err(Failure {
                kind: FailureKind::Other,
                message: "could not reach MELCloud".into(),
            }),
            manual: true,
        });
        assert_eq!(model.activity[0].tone, Tone::Bad);
        assert_eq!(model.next_refresh, Some(now() + RETRY_AFTER_FAILURE));

        model.update(key(KeyCode::Char('r')));
        model.update(Msg::State {
            result: Err(Failure {
                kind: FailureKind::NeedsLogin,
                message: "the MELCloud session expired".into(),
            }),
            manual: true,
        });
        assert!(matches!(model.overlay, Overlay::Login(_)));
        assert_eq!(model.account, None);
    }

    #[test]
    fn newer_state_from_another_process_is_shown_with_what_changed() {
        let (mut model, _) = Model::start(start());
        model.update(Msg::External(state(SignedDuration::from_mins(3))));
        assert_eq!(
            model.state.as_ref().unwrap().fetched_at,
            now() - SignedDuration::from_mins(2)
        );

        // Same values, only newer: shown without a note.
        model.update(Msg::External(state(SignedDuration::from_mins(1))));
        assert_eq!(
            model.state.as_ref().unwrap().fetched_at,
            now() - SignedDuration::from_mins(1)
        );
        assert_eq!(model.activity.len(), 0);

        let mut changed: serde_json::Value = serde_json::from_str(DEVICE_GET).unwrap();
        changed["Power"] = serde_json::json!(false);
        changed["SetTemperature"] = serde_json::json!(23.5);
        let changed = Cached {
            fetched_at: now(),
            value: serde_json::from_value(changed).unwrap(),
        };
        model.update(Msg::External(changed));
        assert_eq!(model.activity[0].text, "another klima: power off, 23.5 °C");
    }

    #[test]
    fn keys_switch_tabs_open_help_and_quit() {
        let (mut model, _) = Model::start(start());
        model.update(key(KeyCode::Char('3')));
        assert_eq!(model.tab, 2);
        model.update(key(KeyCode::Tab));
        model.update(key(KeyCode::Tab));
        assert_eq!(model.tab, 0);
        model.update(key(KeyCode::Char('?')));
        assert_eq!(model.overlay, Overlay::Help);
        model.update(key(KeyCode::Char('q')));
        assert_eq!(model.overlay, Overlay::None);
        assert_eq!(model.quit, Quit::No);
        model.update(key(KeyCode::Char('q')));
        assert_eq!(model.quit, Quit::Now);
    }

    fn press(model: &mut Model, code: KeyCode) -> Vec<Effect> {
        model.update(key(code))
    }

    fn after(model: &mut Model, delay: SignedDuration) -> Vec<Effect> {
        model.update(Msg::Tick(model.now + delay))
    }

    #[test]
    fn changes_merge_for_three_seconds_and_go_out_as_one_command() {
        let (mut model, _) = Model::start(start());
        assert_eq!(press(&mut model, KeyCode::Char('+')), []);
        assert_eq!(after(&mut model, SignedDuration::from_secs(2)), []);
        press(&mut model, KeyCode::Char('f'));
        assert_eq!(model.shown().unwrap().target_temperature, 22.5);
        assert_eq!(
            after(&mut model, SignedDuration::from_millis(2900)),
            [],
            "window restarted"
        );

        let sent = after(&mut model, SignedDuration::from_millis(200));
        let expected = ChangeSet {
            target_temperature: Some(22.5),
            fan_speed: Some(FanSpeed::Level(3)),
            ..ChangeSet::default()
        };
        assert!(
            matches!(sent.as_slice(), [Effect::Send { changes, .. }] if *changes == expected),
            "{sent:?}"
        );
        assert_eq!(model.pending, None);
        assert_eq!(model.flight.map(|f| f.phase), Some(Phase::Sending));
    }

    #[test]
    fn changing_back_leaves_nothing_to_send() {
        let (mut model, _) = Model::start(start());
        press(&mut model, KeyCode::Char('+'));
        press(&mut model, KeyCode::Char('-'));
        assert_eq!(model.pending, None);
        assert_eq!(press(&mut model, KeyCode::Enter), []);
    }

    #[test]
    fn rows_move_and_change_their_setting() {
        let (mut model, _) = Model::start(start());
        press(&mut model, KeyCode::Char('j'));
        assert_eq!(model.row, Row::Mode);
        press(&mut model, KeyCode::Right);
        assert_eq!(model.shown().unwrap().mode, OperationMode::Fan);
        press(&mut model, KeyCode::Down);
        press(&mut model, KeyCode::Down);
        press(&mut model, KeyCode::Down);
        press(&mut model, KeyCode::Down);
        assert_eq!(model.row, Row::VaneHorizontal);
        press(&mut model, KeyCode::Char('l'));
        press(&mut model, KeyCode::Char('l'));
        press(&mut model, KeyCode::Char('l'));
        assert_eq!(
            model.shown().unwrap().vane_horizontal,
            VaneHorizontal::Split
        );
    }

    #[test]
    fn temperature_stops_at_the_range_with_a_note() {
        let (mut model, _) = Model::start(start());
        model.update(Msg::Wheel(40));
        assert_eq!(model.shown().unwrap().target_temperature, 31.0);
        press(&mut model, KeyCode::Char('+'));
        assert_eq!(model.notice.as_ref().unwrap().text, "16-31 °C in cool mode");
    }

    #[test]
    fn switching_to_a_mode_with_another_range_moves_the_temperature_into_it() {
        let (mut model, _) = Model::start(start());
        model.update(Msg::Wheel(-12));
        assert_eq!(model.shown().unwrap().target_temperature, 16.0);
        model.update(Msg::Target(Target::Mode(OperationMode::Heat)));
        model.update(Msg::Wheel(-12));
        assert_eq!(model.shown().unwrap().target_temperature, 10.0);
        model.update(Msg::Target(Target::Mode(OperationMode::Cool)));
        assert_eq!(model.shown().unwrap().target_temperature, 16.0);
    }

    #[test]
    fn a_command_moves_from_sending_to_applied() {
        let (mut model, _) = Model::start(start());
        press(&mut model, KeyCode::Char('P'));
        assert!(matches!(
            press(&mut model, KeyCode::Enter).as_slice(),
            [Effect::Send { .. }]
        ));
        assert!(
            !model.shown().unwrap().power,
            "shown as sent while on its way"
        );

        model.update(Msg::Sent(Ok(())));
        assert_eq!(model.flight.map(|f| f.phase), Some(Phase::Confirming));
        model.update(Msg::Confirmed {
            result: Ok(Some(SignedDuration::from_millis(1100))),
            state: None,
        });
        assert_eq!(model.flight, None);
        let notice = model.notice.as_ref().unwrap();
        assert_eq!(
            notice.text,
            "applied · the adapter took it 1.1 s after sending"
        );
        assert_eq!(model.activity[0].text, "applied: power off");
    }

    #[test]
    fn changes_made_while_a_command_is_on_its_way_follow_it() {
        let (mut model, _) = Model::start(start());
        press(&mut model, KeyCode::Char('+'));
        press(&mut model, KeyCode::Enter);
        press(&mut model, KeyCode::Char('+'));
        assert_eq!(
            model.pending.unwrap().changes.target_temperature,
            Some(23.0)
        );
        assert_eq!(
            after(&mut model, SignedDuration::from_secs(4)),
            [],
            "waits for the first"
        );
        let next = model.update(Msg::Confirmed {
            result: Ok(None),
            state: None,
        });
        assert!(matches!(next.as_slice(), [Effect::Send { .. }]), "{next:?}");
    }

    #[test]
    fn quitting_with_waiting_changes_asks_first() {
        let (mut model, _) = Model::start(start());
        press(&mut model, KeyCode::Char('m'));
        press(&mut model, KeyCode::Char('q'));
        assert_eq!(model.quit, Quit::Asking);
        press(&mut model, KeyCode::Esc);
        assert_eq!(model.quit, Quit::No);

        press(&mut model, KeyCode::Char('q'));
        assert!(matches!(
            press(&mut model, KeyCode::Enter).as_slice(),
            [Effect::Send { .. }]
        ));
        assert_eq!(
            model.quit,
            Quit::AfterSend,
            "waits for the server to take it"
        );
        model.update(Msg::Sent(Ok(())));
        assert_eq!(model.quit, Quit::Now);
    }

    #[test]
    fn waiting_changes_are_not_sent_while_quitting_is_asked_about() {
        let (mut model, _) = Model::start(start());
        press(&mut model, KeyCode::Char('m'));
        press(&mut model, KeyCode::Char('q'));
        assert_eq!(after(&mut model, SignedDuration::from_secs(10)), []);
        assert!(model.pending.is_some() && model.quit == Quit::Asking);
        press(&mut model, KeyCode::Esc);
        assert!(matches!(
            after(&mut model, SignedDuration::ZERO).as_slice(),
            [Effect::Send { .. }]
        ));
    }

    #[test]
    fn discarding_drops_the_waiting_changes() {
        let (mut model, _) = Model::start(start());
        press(&mut model, KeyCode::Char('m'));
        press(&mut model, KeyCode::Char('u'));
        assert_eq!(model.pending, None);
        assert_eq!(model.activity[0].text, "discarded: mode fan");
    }

    #[test]
    fn a_guest_without_the_permission_gets_a_note() {
        let (mut model, _) = Model::start(Start {
            preferred_device: Some("Bedroom".into()),
            ..start()
        });
        press(&mut model, KeyCode::Char('f'));
        assert_eq!(model.pending, None);
        assert_eq!(
            model.notice.as_ref().unwrap().text,
            "this account may not change the fan speed"
        );
    }

    #[test]
    fn no_automatic_refresh_while_changes_wait() {
        let (mut model, _) = Model::start(start());
        press(&mut model, KeyCode::Char('m'));
        model.pending = Some(Pending {
            deadline: now() + SignedDuration::from_hours(1),
            ..model.pending.unwrap()
        });
        assert_eq!(after(&mut model, SignedDuration::from_mins(30)), []);
    }

    #[test]
    fn digits_flash_when_the_target_changes() {
        let (mut model, _) = Model::start(start());
        assert_eq!(model.flash_until, None);
        press(&mut model, KeyCode::Char('+'));
        assert_eq!(model.flash_until, Some(now() + FLASH));
        assert!(model.animating());
        model.flash_until = None;
        press(&mut model, KeyCode::Char('m'));
        assert_eq!(
            model.flash_until, None,
            "a mode change alone does not flash"
        );
        press(&mut model, KeyCode::Char('u'));
        assert_eq!(
            model.flash_until,
            Some(now() + FLASH),
            "discarding brings the old target back"
        );

        let (mut still, _) = Model::start(Start {
            animations: false,
            ..start()
        });
        press(&mut still, KeyCode::Char('+'));
        assert_eq!(still.flash_until, None);
        assert!(!still.moving());
    }

    #[test]
    fn a_running_unit_is_drawn_in_motion() {
        let (mut model, _) = Model::start(start());
        assert!(model.moving());
        press(&mut model, KeyCode::Char('P'));
        assert!(!model.moving(), "shown as off once switched off");
    }

    #[test]
    fn cycling_wraps_and_starts_over_when_lost() {
        assert_eq!(cycle(&[1, 2, 3], 3, 1), Some(1));
        assert_eq!(cycle(&[1, 2, 3], 1, -1), Some(3));
        assert_eq!(cycle(&[1, 2, 3], 9, 1), Some(1));
        assert_eq!(cycle::<i32>(&[], 1, 1), None);
    }

    #[test]
    fn preferred_device_is_chosen_by_name_or_id() {
        let list = devices();
        assert_eq!(choose(list.value.devices(), None), Some(0));
        assert_eq!(choose(list.value.devices(), Some("bedroom")), Some(1));
        assert_eq!(choose(list.value.devices(), Some("1000002")), Some(1));
        assert_eq!(choose(list.value.devices(), Some("garage")), Some(0));
    }
}
