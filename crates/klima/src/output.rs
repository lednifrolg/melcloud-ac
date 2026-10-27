//! What klima prints. Colors use the terminal's ANSI palette, so they follow the terminal theme,
//! and disappear when the output is not a terminal or `NO_COLOR` is set.

use std::error::Error as _;
use std::io::{self, IsTerminal, Write};

use anstream::{eprint, eprintln, println};
use anstyle::{AnsiColor, Style};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use melcloud::{
    AtaState, Capabilities, Device, DeviceId, DeviceKind, EffectiveFlags, FanSpeed, OperationMode,
    WeatherKind,
};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::AppError;

const DIM: Style = Style::new().dimmed();
const BOLD: Style = Style::new().bold();
const GREEN: Style = AnsiColor::Green.on_default();
const YELLOW: Style = AnsiColor::Yellow.on_default();
const RED: Style = AnsiColor::Red.on_default();
const CYAN: Style = AnsiColor::Cyan.on_default();

/// The color that stands for a mode, as in the TUI.
fn accent(mode: OperationMode) -> Style {
    let color = match mode {
        OperationMode::Heat => AnsiColor::BrightYellow,
        OperationMode::Dry => AnsiColor::Cyan,
        OperationMode::Cool => AnsiColor::BrightCyan,
        OperationMode::Auto => AnsiColor::BrightMagenta,
        OperationMode::Fan | OperationMode::Other(_) => AnsiColor::White,
    };
    color.on_default()
}

fn paint(style: Style, text: impl std::fmt::Display) -> String {
    format!("{style}{text}{style:#}")
}

/// A time in the local zone: just the clock for today, with the date otherwise.
pub fn local_time(at: Timestamp) -> String {
    let zone = TimeZone::system();
    let zoned = at.to_zoned(zone.clone());
    let today = Timestamp::now().to_zoned(zone).date();
    if zoned.date() == today {
        zoned.strftime("%H:%M").to_string()
    } else {
        zoned.strftime("%Y-%m-%d %H:%M").to_string()
    }
}

/// Whole seconds until `at`, rounded up; 0 once it has passed.
pub fn seconds_until(at: Timestamp) -> u64 {
    let millis = u128::try_from(at.duration_since(Timestamp::now()).as_millis()).unwrap_or(0);
    u64::try_from(millis.div_ceil(1000)).unwrap_or(u64::MAX)
}

/// "just now", "42 s ago", "5 min ago", "3 h ago", "2 d ago".
pub fn ago(at: Timestamp, now: Timestamp) -> String {
    let seconds = now.duration_since(at).as_secs().max(0);
    match seconds {
        0..10 => "just now".to_owned(),
        10..60 => format!("{seconds} s ago"),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

/// "45 min", "12 h", "30 days", rounded to the nearest unit.
pub fn duration(span: SignedDuration) -> String {
    let seconds = span.as_secs().max(0);
    let nearest = |unit: i64| (seconds + unit / 2) / unit;
    match seconds {
        0..7200 => format!("{} min", nearest(60)),
        7200..172_800 => format!("{} h", nearest(3600)),
        _ => format!("{} days", nearest(86_400)),
    }
}

pub fn error(error: &AppError) {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        let text = inner.to_string();
        if !message.contains(&text) {
            message = format!("{message}: {text}");
        }
        cause = inner.source();
    }
    eprintln!("{} {message}", paint(RED.bold(), "error:"));
}

pub fn note(text: impl std::fmt::Display) {
    eprintln!("{}", paint(DIM, text));
}

pub fn success(text: impl std::fmt::Display) {
    println!("{} {text}", paint(GREEN, "✓"));
}

pub fn json(value: &impl Serialize) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("output is plain data")
    );
}

fn fan_bars(fan: FanSpeed, levels: u8, style: Style) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let top = usize::from(levels.max(2)) - 1;
    (1..=levels)
        .map(|level| {
            let block = BLOCKS[(usize::from(level) - 1) * 7 / top];
            let lit = match fan {
                FanSpeed::Auto => true,
                FanSpeed::Level(current) => level <= current,
                FanSpeed::Other(_) => false,
            };
            paint(if lit { style } else { DIM }, block)
        })
        .collect()
}

/// Where a shown state came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Server,
    Cache,
}

/// A unit state to show, with how it was obtained.
pub struct StatusReport<'a> {
    pub device: &'a Device,
    pub state: &'a AtaState,
    pub fetched_at: Timestamp,
    pub source: Source,
    /// Why the state could not be refreshed, when it is older than it should be.
    pub stale_because: Option<String>,
}

pub fn status(report: &StatusReport, caps: &Capabilities) {
    let view = report.state.view();
    let tone = if view.power { accent(view.mode) } else { DIM };
    let model = report
        .device
        .model
        .as_deref()
        .map(|m| format!(" · {m}"))
        .unwrap_or_default();
    let power = if view.power {
        paint(GREEN, "● on")
    } else {
        paint(DIM, "○ off")
    };
    println!(
        "{}{}   {power} {} {}",
        paint(BOLD, &report.device.name),
        paint(DIM, model),
        paint(DIM, "·"),
        paint(tone.bold(), view.mode)
    );

    let outside = view
        .weather
        .iter()
        .find(|w| w.kind == WeatherKind::Current)
        .map(|w| format!("   {:.0} °C {}", w.temperature, paint(DIM, "outside")))
        .unwrap_or_default();
    println!(
        "{} {}   {:.1} °C {}{outside}",
        paint(tone.bold(), format!("{:.1} °C", view.target_temperature)),
        paint(DIM, "target"),
        view.room_temperature,
        paint(DIM, "room"),
    );

    let mut controls = Vec::new();
    if caps.fan_levels > 0 {
        let level = match view.fan_speed {
            FanSpeed::Level(level) => format!("{level}/{}", caps.fan_levels),
            other => other.to_string(),
        };
        controls.push(format!(
            "{} {} {level}",
            paint(DIM, "fan"),
            fan_bars(view.fan_speed, caps.fan_levels, tone)
        ));
    }
    if caps.vane_vertical {
        controls.push(format!("{} {}", paint(DIM, "vane ↕"), view.vane_vertical));
    }
    if caps.vane_horizontal {
        controls.push(format!("{} {}", paint(DIM, "↔"), view.vane_horizontal));
    }
    if !controls.is_empty() {
        println!("{}", controls.join("   "));
    }

    if let Some(reason) = &report.stale_because {
        println!("{}", paint(YELLOW, format!("! not refreshed: {reason}")));
    }
    if view.has_error() {
        let detail = view
            .error_message
            .as_deref()
            .map(|m| format!(": {m}"))
            .unwrap_or_default();
        println!(
            "{}",
            paint(RED, format!("! unit error {}{detail}", view.error_code))
        );
    }
    if !view.effective_flags.is_empty() {
        println!(
            "{}",
            paint(YELLOW, "! the unit has not applied the last command yet")
        );
    }
    if view.offline {
        println!(
            "{}",
            paint(
                YELLOW,
                "! the adapter missed its last check-in; commands still wake it up"
            )
        );
    }
    let source = match report.source {
        Source::Server => "",
        Source::Cache => " · cached",
    };
    println!(
        "{}",
        paint(
            DIM,
            format!(
                "updated {}{source}",
                ago(report.fetched_at, Timestamp::now())
            )
        )
    );
}

#[derive(Serialize)]
#[expect(clippy::struct_excessive_bools, reason = "mirrors the unit's flags")]
struct StatusJson<'a> {
    device: DeviceRef<'a>,
    power: bool,
    mode: String,
    target_temperature: f64,
    room_temperature: f64,
    outside_temperature: Option<f64>,
    fan_speed: String,
    vane_vertical: String,
    vane_horizontal: String,
    offline: bool,
    pending_command: bool,
    error_code: i32,
    error_message: Option<&'a str>,
    last_communication: Option<Timestamp>,
    fetched_at: Timestamp,
    source: &'static str,
    stale: bool,
}

#[derive(Serialize)]
struct DeviceRef<'a> {
    id: DeviceId,
    name: &'a str,
}

pub fn status_json(report: &StatusReport) {
    let view = report.state.view();
    json(&StatusJson {
        device: DeviceRef {
            id: report.device.id,
            name: &report.device.name,
        },
        power: view.power,
        mode: view.mode.to_string(),
        target_temperature: view.target_temperature,
        room_temperature: view.room_temperature,
        outside_temperature: view
            .weather
            .iter()
            .find(|w| w.kind == WeatherKind::Current)
            .map(|w| w.temperature),
        fan_speed: view.fan_speed.to_string(),
        vane_vertical: view.vane_vertical.to_string(),
        vane_horizontal: view.vane_horizontal.to_string(),
        offline: view.offline,
        pending_command: !view.effective_flags.is_empty(),
        error_code: view.error_code,
        error_message: view.error_message.as_deref(),
        last_communication: view.last_communication,
        fetched_at: report.fetched_at,
        source: match report.source {
            Source::Server => "server",
            Source::Cache => "cache",
        },
        stale: report.stale_because.is_some(),
    });
}

fn kind_name(kind: DeviceKind) -> String {
    match kind {
        DeviceKind::AirToAir => "air conditioner".into(),
        DeviceKind::AirToWater => "heat pump".into(),
        DeviceKind::Ventilation => "ventilation".into(),
        DeviceKind::Other(code) => format!("device type {code}"),
    }
}

pub fn devices(devices: &[Device], fetched_at: Timestamp) {
    let header = ["NAME", "MODEL", "ACCESS", "ID", "BUILDING"];
    let rows: Vec<[String; 5]> = devices
        .iter()
        .map(|d| {
            [
                d.name.clone(),
                d.model.clone().unwrap_or_else(|| kind_name(d.kind)),
                (if d.is_owner() { "owner" } else { "guest" }).to_owned(),
                d.id.to_string(),
                d.building_name.clone(),
            ]
        })
        .collect();
    let width = |column: usize| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .chain([header[column].len()])
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<usize> = (0..header.len()).map(width).collect();
    let line = |cells: &[&str]| {
        cells
            .iter()
            .zip(&widths)
            .map(|(cell, &w)| format!("{cell:<w$}"))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };
    println!("{}", paint(DIM, line(&header)));
    for row in &rows {
        println!("{}", line(&row.each_ref().map(String::as_str)));
    }
    println!(
        "{}",
        paint(
            DIM,
            format!("updated {}", ago(fetched_at, Timestamp::now()))
        )
    );
}

#[derive(Serialize)]
struct DeviceJson<'a> {
    id: DeviceId,
    name: &'a str,
    model: Option<&'a str>,
    kind: String,
    owner: bool,
    building_id: melcloud::BuildingId,
    building: &'a str,
    instant_delivery: bool,
    capabilities: Option<CapabilitiesJson>,
}

#[derive(Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per optional feature"
)]
struct CapabilitiesJson {
    modes: Vec<String>,
    fan_levels: u8,
    fan_auto: bool,
    vane_vertical: bool,
    vane_horizontal: bool,
    wide_vane: bool,
    temperature_step: f64,
    heat_range: [f64; 2],
    cool_range: [f64; 2],
    auto_range: [f64; 2],
}

pub fn devices_json(devices: &[Device]) {
    let list: Vec<_> = devices
        .iter()
        .map(|d| DeviceJson {
            id: d.id,
            name: &d.name,
            model: d.model.as_deref(),
            kind: kind_name(d.kind),
            owner: d.is_owner(),
            building_id: d.building_id,
            building: &d.building_name,
            instant_delivery: d.instant_delivery,
            capabilities: d.capabilities.as_ref().map(|c| CapabilitiesJson {
                modes: c.modes.iter().map(ToString::to_string).collect(),
                fan_levels: c.fan_levels,
                fan_auto: c.fan_auto,
                vane_vertical: c.vane_vertical,
                vane_horizontal: c.vane_horizontal,
                wide_vane: c.wide_vane,
                temperature_step: c.temperature_step,
                heat_range: [c.heat_range.min, c.heat_range.max],
                cool_range: [c.cool_range.min, c.cool_range.max],
                auto_range: [c.auto_range.min, c.auto_range.max],
            }),
        })
        .collect();
    json(&list);
}

/// One setting a command changes, for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub field: &'static str,
    pub from: String,
    pub to: String,
}

/// What a command did, for `--json`.
pub struct CommandReport<'a> {
    pub device: &'a Device,
    pub changes: &'a [Change],
    pub flags: Option<EffectiveFlags>,
    pub dry_run: bool,
    pub sent: bool,
    pub adapter_delay: Option<SignedDuration>,
    /// The JSON that would be sent; only for `--dry-run`.
    pub body: Option<&'a Map<String, Value>>,
}

/// Width of the "✓ sent" label column, so the lines of a command line up.
const LABEL: usize = 10;

fn label(style: Style, text: &str) -> String {
    paint(style, format!("{text:<LABEL$}"))
}

fn changes_line(changes: &[Change]) -> String {
    changes
        .iter()
        .map(|c| {
            format!(
                "{} {} {} {}",
                paint(DIM, c.field),
                c.from,
                paint(DIM, "→"),
                paint(BOLD, &c.to)
            )
        })
        .collect::<Vec<_>>()
        .join(&paint(DIM, " · "))
}

fn flags_line(flags: EffectiveFlags) -> String {
    paint(
        DIM,
        format!("{:LABEL$}SetAta, flags 0x{:02X}", "", flags.bits()),
    )
}

pub fn would_send(changes: &[Change], flags: EffectiveFlags) {
    println!("{}{}", label(YELLOW, "→ dry run"), changes_line(changes));
    println!("{}", flags_line(flags));
}

pub fn sent(changes: &[Change], flags: EffectiveFlags) {
    println!("{}{}", label(GREEN, "✓ sent"), changes_line(changes));
    println!("{}", flags_line(flags));
}

pub fn applied(adapter_delay: Option<SignedDuration>) {
    let detail = match adapter_delay {
        Some(delay) => format!(
            "the adapter took it {:.1} s after sending",
            delay.as_secs_f64()
        ),
        None => "the adapter confirmed it".to_owned(),
    };
    println!("{}{detail}", label(GREEN, "✓ applied"));
}

/// Waits until `until`, with a spinner on stderr when it is a terminal and `show` is set.
pub async fn wait_for_adapter(until: Timestamp, since: Timestamp, show: bool) {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let spinner = show && io::stderr().is_terminal();
    let mut frame = 0;
    loop {
        let now = Timestamp::now();
        let left = until.duration_since(now);
        if left <= SignedDuration::ZERO {
            break;
        }
        if spinner {
            let elapsed = now.duration_since(since).as_secs_f64();
            eprint!(
                "\r{} waiting for the adapter · {elapsed:.1} s",
                paint(CYAN, FRAMES[frame % FRAMES.len()])
            );
            frame += 1;
        }
        tokio::time::sleep(left.min(SignedDuration::from_millis(80)).unsigned_abs()).await;
    }
    if spinner {
        let _ = io::stderr().write_all(b"\r\x1b[2K");
    }
}

#[derive(Serialize)]
struct CommandJson<'a> {
    device: DeviceRef<'a>,
    changes: &'a [Change],
    flags: Option<u64>,
    dry_run: bool,
    sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    confirmed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    adapter_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a Map<String, Value>>,
}

/// `outcome` is the result of waiting for the adapter, when the command waited for it.
pub fn command_json(
    report: &CommandReport,
    outcome: Option<&Result<Option<SignedDuration>, AppError>>,
) {
    json(&CommandJson {
        device: DeviceRef {
            id: report.device.id,
            name: &report.device.name,
        },
        changes: report.changes,
        flags: report.flags.map(|f| f.bits()),
        dry_run: report.dry_run,
        sent: report.sent,
        confirmed: outcome.map(Result::is_ok),
        adapter_seconds: report
            .adapter_delay
            .map(|d| (d.as_secs_f64() * 10.0).round() / 10.0),
        error: outcome
            .and_then(|o| o.as_ref().err())
            .map(ToString::to_string),
        body: report.body,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_naturally() {
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let before = |seconds: i64| now - SignedDuration::from_secs(seconds);
        assert_eq!(ago(before(3), now), "just now");
        assert_eq!(ago(before(42), now), "42 s ago");
        assert_eq!(ago(before(5 * 60 + 59), now), "5 min ago");
        assert_eq!(ago(before(3 * 3600), now), "3 h ago");
        assert_eq!(ago(before(2 * 86_400), now), "2 d ago");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(SignedDuration::from_mins(30)), "30 min");
        assert_eq!(duration(SignedDuration::from_hours(12)), "12 h");
        assert_eq!(duration(SignedDuration::from_hours(24 * 365)), "365 days");
        let almost_a_month = SignedDuration::from_hours(24 * 30) - SignedDuration::from_secs(2);
        assert_eq!(duration(almost_a_month), "30 days");
    }

    #[test]
    fn fan_bars_light_up_to_the_level() {
        let dim = DIM.render().to_string();
        let lit = BOLD.render().to_string();
        let bars = fan_bars(FanSpeed::Level(2), 5, BOLD);
        assert_eq!(
            (bars.matches(&lit).count(), bars.matches(&dim).count()),
            (2, 3)
        );
        let glyphs: String = bars.chars().filter(|c| ('▁'..='█').contains(c)).collect();
        assert_eq!(glyphs, "▁▂▄▆█");
        assert_eq!(fan_bars(FanSpeed::Auto, 4, BOLD).matches(&dim).count(), 0);
    }
}
