//! Commands that change the unit: `on`, `off`, `toggle`, `temp`, `mode`, `fan`, `vane`, `set`.

use std::str::FromStr;

use jiff::{SignedDuration, Timestamp};
use melcloud::{
    AtaState, AtaView, ChangeSet, Device, FanSpeed, OperationMode, VaneHorizontal, VaneVertical,
};

use crate::app::App;
use crate::commands::select;
use crate::error::AppError;
use crate::governor::Request;
use crate::output::{self, Change, CommandReport};

/// With MQTT the adapter gets a command within about a second, so the first check comes after
/// this long.
const INSTANT_CHECK_AFTER: SignedDuration = SignedDuration::from_secs(3);
/// Without MQTT the adapter picks the command up at its next contact; check this long after it.
const CONTACT_MARGIN: SignedDuration = SignedDuration::from_secs(10);
/// No single wait for the adapter is longer than this.
const LONGEST_WAIT: SignedDuration = SignedDuration::from_secs(90);
/// Reads that check whether the unit applied a command. The command itself is never resent.
const CHECKS: usize = 2;

/// What to do with the power.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Power {
    On,
    Off,
    /// The opposite of what the unit does now.
    Toggle,
}

/// A target temperature in °C: absolute, or relative to the current target when it starts with
/// `+` or `-`. A decimal comma works too.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Temperature {
    Absolute(f64),
    Relative(f64),
}

impl FromStr for Temperature {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let text = s.trim().replace(',', ".");
        let value = text
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("\"{s}\" is not a temperature; use e.g. 22.5, +1 or -0.5"))?;
        Ok(if text.starts_with(['+', '-']) {
            Self::Relative(value)
        } else {
            Self::Absolute(value)
        })
    }
}

/// Options shared by every command that changes the unit.
#[derive(Debug, Clone, Copy, clap::Args)]
pub struct Options {
    /// Show what would be sent, without sending it.
    #[arg(long)]
    pub dry_run: bool,
    /// Do not wait for the adapter to confirm the command.
    #[arg(long, conflicts_with = "dry_run")]
    pub no_wait: bool,
}

/// What the user asked for.
#[derive(Debug, Clone, Copy, Default)]
pub struct Wanted {
    pub power: Option<Power>,
    pub mode: Option<OperationMode>,
    pub temperature: Option<Temperature>,
    pub fan: Option<FanSpeed>,
    pub vane_vertical: Option<VaneVertical>,
    pub vane_horizontal: Option<VaneHorizontal>,
}

impl Wanted {
    /// The requested settings in absolute terms, given the unit's current state.
    fn settings(&self, current: &AtaView) -> ChangeSet {
        ChangeSet {
            power: self.power.map(|power| match power {
                Power::On => true,
                Power::Off => false,
                Power::Toggle => !current.power,
            }),
            mode: self.mode,
            target_temperature: self.temperature.map(|temperature| match temperature {
                Temperature::Absolute(celsius) => celsius,
                Temperature::Relative(delta) => current.target_temperature + delta,
            }),
            fan_speed: self.fan,
            vane_vertical: self.vane_vertical,
            vane_horizontal: self.vane_horizontal,
        }
    }
}

/// Leaves out the settings the unit already has, so nothing is sent for them.
pub fn changes(settings: ChangeSet, current: &AtaView) -> ChangeSet {
    ChangeSet {
        power: settings.power.filter(|&p| p != current.power),
        mode: settings.mode.filter(|&m| m != current.mode),
        target_temperature: settings
            .target_temperature
            .filter(|&t| (t - current.target_temperature).abs() > 1e-6),
        fan_speed: settings.fan_speed.filter(|&f| f != current.fan_speed),
        vane_vertical: settings
            .vane_vertical
            .filter(|&v| v != current.vane_vertical),
        vane_horizontal: settings
            .vane_horizontal
            .filter(|&v| v != current.vane_horizontal),
    }
}

fn on_off(power: bool) -> String {
    (if power { "on" } else { "off" }).to_owned()
}

fn celsius(value: f64) -> String {
    format!("{value:.1} °C")
}

/// Each set field of `changes`, with its current value.
pub fn describe(changes: &ChangeSet, current: &AtaView) -> Vec<Change> {
    let mut described = Vec::new();
    let mut add = |field, from: String, to: String| described.push(Change { field, from, to });
    if let Some(power) = changes.power {
        add("power", on_off(current.power), on_off(power));
    }
    if let Some(mode) = changes.mode {
        add("mode", current.mode.to_string(), mode.to_string());
    }
    if let Some(target) = changes.target_temperature {
        add("temp", celsius(current.target_temperature), celsius(target));
    }
    if let Some(fan) = changes.fan_speed {
        add("fan", current.fan_speed.to_string(), fan.to_string());
    }
    if let Some(vane) = changes.vane_vertical {
        add(
            "vane ↕",
            current.vane_vertical.to_string(),
            vane.to_string(),
        );
    }
    if let Some(vane) = changes.vane_horizontal {
        add(
            "vane ↔",
            current.vane_horizontal.to_string(),
            vane.to_string(),
        );
    }
    described
}

/// The changed fields whose value in `applied` differs from what was asked.
fn mismatches(changes: &ChangeSet, applied: &AtaView) -> Vec<String> {
    describe(changes, applied)
        .into_iter()
        .filter(|change| change.from != change.to)
        .map(|change| format!("{} {} instead of {}", change.field, change.from, change.to))
        .collect()
}

/// Sends what was asked as one command and, unless told not to, waits until the adapter has
/// applied it.
pub async fn run(
    app: &App,
    device: Option<&str>,
    wanted: Wanted,
    options: Options,
    json: bool,
) -> Result<(), AppError> {
    let list = app.devices(false).await?;
    let device = select(
        list.value.devices(),
        device.or(app.config.device.as_deref()),
    )?;
    let base = app.base_state(device).await?;
    let current = base.value.view();
    let settings = wanted.settings(current);
    let changes = changes(settings, current);
    let described = describe(&changes, current);
    let mut report = CommandReport {
        device,
        changes: &described,
        flags: None,
        dry_run: options.dry_run,
        sent: false,
        adapter_delay: None,
        body: None,
    };

    if changes.is_empty() {
        let asked: Vec<String> = describe(&settings, current)
            .into_iter()
            .map(|change| format!("{} {}", change.field, change.to))
            .collect();
        if json {
            output::command_json(&report, None);
        } else {
            output::note(format!(
                "Nothing to send: the unit already has {}.",
                asked.join(", ")
            ));
        }
        return Ok(());
    }

    let command = changes
        .build(device, &base.value)
        .map_err(AppError::Invalid)?;
    report.flags = Some(command.flags());
    if options.dry_run {
        report.body = Some(command.body());
        if json {
            output::command_json(&report, None);
        } else {
            output::would_send(&described, command.flags());
        }
        return Ok(());
    }

    let sent_at = Timestamp::now();
    let answer = app.send(device, &command).await?;
    report.sent = true;
    if !json {
        output::sent(&described, command.flags());
    }
    if options.no_wait {
        if json {
            output::command_json(&report, None);
        }
        return Ok(());
    }

    let outcome = confirm(app, device, &changes, &answer, sent_at, !json).await;
    if let Ok(delay) = &outcome {
        report.adapter_delay = *delay;
    }
    if json {
        output::command_json(&report, Some(&outcome));
    } else if let Ok(delay) = outcome {
        output::applied(delay);
    }
    outcome.map(|_| ())
}

/// Waits for the adapter and checks the unit applied `changes`. Returns how long after sending
/// the adapter picked the command up, when the clocks allow telling.
pub async fn confirm(
    app: &App,
    device: &Device,
    changes: &ChangeSet,
    answer: &AtaState,
    sent_at: Timestamp,
    show_progress: bool,
) -> Result<Option<SignedDuration>, AppError> {
    // The adapter has to talk to the cloud after the server took the command. The server's own
    // timestamp is the reference, so a skewed local clock cannot confirm too early.
    let taken_after = answer.view().last_communication.unwrap_or(sent_at);
    let mut check_at = if device.instant_delivery {
        sent_at + INSTANT_CHECK_AFTER
    } else {
        next_check(answer.view())
    };
    for _ in 0..CHECKS {
        let until = check_at.min(Timestamp::now() + LONGEST_WAIT);
        output::wait_for_adapter(until, sent_at, show_progress).await;
        let state = app.read_state(device, Request::Confirm(device.id)).await?;
        let view = state.value.view();
        if view.confirms(taken_after) {
            let wrong = mismatches(changes, view);
            if !wrong.is_empty() {
                return Err(AppError::NotApplied(wrong.join(", ")));
            }
            let delay = view
                .last_communication
                .map(|contact| contact.duration_since(sent_at))
                .filter(|delay| (SignedDuration::ZERO..=LONGEST_WAIT).contains(delay));
            return Ok(delay);
        }
        check_at = next_check(view);
    }
    let waited = Timestamp::now().duration_since(sent_at).as_secs();
    Err(AppError::NotConfirmed {
        seconds: u64::try_from(waited).unwrap_or(0),
    })
}

/// When to check again: shortly after the adapter's next scheduled contact.
fn next_check(view: &AtaView) -> Timestamp {
    view.next_communication.map_or_else(
        || Timestamp::now() + LONGEST_WAIT,
        |contact| {
            contact
                .checked_add(CONTACT_MARGIN)
                .unwrap_or(Timestamp::MAX)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE_GET: &str = include_str!("../../melcloud/tests/data/device-get.json");

    fn current() -> AtaView {
        serde_json::from_str::<AtaState>(DEVICE_GET)
            .unwrap()
            .view()
            .clone()
    }

    #[test]
    fn temperatures_parse_absolute_and_relative() {
        assert_eq!("22.5".parse(), Ok(Temperature::Absolute(22.5)));
        assert_eq!("22,5".parse(), Ok(Temperature::Absolute(22.5)));
        assert_eq!("+1".parse(), Ok(Temperature::Relative(1.0)));
        assert_eq!("-0.5".parse(), Ok(Temperature::Relative(-0.5)));
        assert!("warm".parse::<Temperature>().is_err());
        assert!("NaN".parse::<Temperature>().is_err());
    }

    #[test]
    fn relative_temperature_and_toggle_follow_the_current_state() {
        let current = current();
        let wanted = Wanted {
            power: Some(Power::Toggle),
            temperature: Some(Temperature::Relative(-1.5)),
            ..Wanted::default()
        };
        let settings = wanted.settings(&current);
        assert_eq!(settings.power, Some(false));
        assert_eq!(settings.target_temperature, Some(20.5));
    }

    #[test]
    fn settings_the_unit_already_has_are_left_out() {
        let current = current();
        let wanted = Wanted {
            power: Some(Power::On),
            mode: Some(OperationMode::Cool),
            temperature: Some(Temperature::Absolute(22.0)),
            fan: Some(FanSpeed::Auto),
            ..Wanted::default()
        };
        let changes = changes(wanted.settings(&current), &current);
        assert_eq!(
            changes,
            ChangeSet {
                fan_speed: Some(FanSpeed::Auto),
                ..ChangeSet::default()
            }
        );
    }

    #[test]
    fn changes_are_described_from_and_to() {
        let current = current();
        let changes = ChangeSet {
            target_temperature: Some(23.0),
            fan_speed: Some(FanSpeed::Auto),
            ..ChangeSet::default()
        };
        let described: Vec<_> = describe(&changes, &current)
            .into_iter()
            .map(|c| format!("{} {} → {}", c.field, c.from, c.to))
            .collect();
        assert_eq!(described, ["temp 22.0 °C → 23.0 °C", "fan 2 → auto"]);
    }

    #[test]
    fn values_the_unit_did_not_take_are_reported() {
        let current = current();
        let changes = ChangeSet {
            target_temperature: Some(23.0),
            fan_speed: Some(FanSpeed::Level(2)),
            ..ChangeSet::default()
        };
        assert_eq!(
            mismatches(&changes, &current),
            ["temp 22.0 °C instead of 23.0 °C"]
        );
    }
}
