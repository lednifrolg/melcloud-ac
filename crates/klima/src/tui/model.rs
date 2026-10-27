//! What the TUI knows and how messages change it. Pure: I/O happens in [`super`], so this is
//! tested directly.

use std::collections::VecDeque;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use melcloud::{AtaState, AtaView, Device, DeviceList};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::keymap::{self, Action};
use crate::cache::{Cached, STATE_TTL};

/// Panes as tabs, for narrow windows.
pub const TABS: [&str; 4] = ["control", "info", "history", "activity"];
const ACTIVITY_LIMIT: usize = 100;
/// After a failure that is not about limits, try again this soon.
const RETRY_AFTER_FAILURE: SignedDuration = SignedDuration::from_mins(2);

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
    /// Another klima process stored a newer state.
    External(Cached<AtaState>),
    Budget(usize),
    History(Vec<(Timestamp, f64)>),
}

/// Background work the TUI asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    LoadDevices,
    Refresh { device: Box<Device>, manual: bool },
    Login { email: String, password: String },
}

/// Everything the TUI starts with, read from disk.
pub struct Start {
    pub now: Timestamp,
    pub tz: TimeZone,
    pub refresh_every: SignedDuration,
    pub preferred_device: Option<String>,
    pub devices: Option<Cached<DeviceList>>,
    pub state: Option<Cached<AtaState>>,
    pub history: Vec<(Timestamp, f64)>,
    pub requests_last_hour: usize,
    pub account: Option<Account>,
    /// Offered in the login form.
    pub email: Option<String>,
}

pub struct Model {
    pub now: Timestamp,
    pub tz: TimeZone,
    pub refresh_every: SignedDuration,
    pub devices: Option<Cached<DeviceList>>,
    pub device: Option<usize>,
    pub state: Option<Cached<AtaState>>,
    pub history: Vec<(Timestamp, f64)>,
    pub busy: Option<Busy>,
    pub next_refresh: Option<Timestamp>,
    pub requests_last_hour: usize,
    pub account: Option<Account>,
    pub activity: VecDeque<Entry>,
    pub overlay: Overlay,
    pub tab: usize,
    pub quit: bool,
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
            quit: false,
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

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
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
                if newer && self.busy != Some(Busy::State) {
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
            Msg::Budget(requests) => self.requests_last_hour = requests,
            Msg::History(history) => self.history = history,
        }
        self.due()
    }

    /// Shows `state` and schedules the next automatic refresh after it.
    fn adopt(&mut self, state: Cached<AtaState>) {
        self.next_refresh = Some(state.fetched_at + self.refresh_every);
        self.state = Some(state);
    }

    /// Starts the background work that is due: the device list first, then a state read once
    /// the cached state is too old.
    fn due(&mut self) -> Vec<Effect> {
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
            self.quit = true;
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
        match keymap::action(key) {
            Some(Action::Refresh) if self.account.is_none() => self.open_login(),
            Some(Action::Refresh) => return self.refresh(true),
            Some(Action::Login) => self.open_login(),
            Some(Action::NextTab) => self.tab = (self.tab + 1) % TABS.len(),
            Some(Action::Tab(index)) if index < TABS.len() => self.tab = index,
            Some(Action::Help) => self.overlay = Overlay::Help,
            Some(Action::Quit) => self.quit = true,
            Some(Action::Tab(_)) | None => {}
        }
        Vec::new()
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
        assert!(!model.quit);
        model.update(key(KeyCode::Char('q')));
        assert!(model.quit);
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
