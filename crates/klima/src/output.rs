//! What klima prints. Colors use the terminal's ANSI palette, so they follow the terminal theme,
//! and disappear when the output is not a terminal or `NO_COLOR` is set.

use std::error::Error as _;
use std::io::{self, IsTerminal, Write};

use anstream::{eprint, eprintln, println};
use anstyle::{Ansi256Color, AnsiColor, Style};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use melcloud::{
    AtaState, Capabilities, Device, DeviceId, DeviceKind, EffectiveFlags, FanSpeed, OperationMode,
    WeatherKind,
};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::cache::Sample;
use crate::chart::{Column, GAP, Timeline, cell};
use crate::error::AppError;
use crate::theme;

const DIM: Style = Style::new().dimmed();
const BOLD: Style = Style::new().bold();
const GREEN: Style = AnsiColor::Green.on_default();
const YELLOW: Style = AnsiColor::Yellow.on_default();
const RED: Style = AnsiColor::Red.on_default();
const CYAN: Style = AnsiColor::Cyan.on_default();
/// The room temperature in history charts, as in the TUI.
const ROOM: Style = AnsiColor::Blue.on_default();

/// The color that stands for a mode, as in the TUI.
fn accent(mode: OperationMode) -> Style {
    let color = Ansi256Color(theme::mode_color(mode));
    color
        .into_ansi()
        .map_or_else(|| color.on_default(), AnsiColor::on_default)
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

/// The error with the causes that add something, in one line.
pub fn message(error: &AppError) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        let text = inner.to_string();
        if !message.contains(&text) {
            message = format!("{message}: {text}");
        }
        cause = inner.source();
    }
    message
}

pub fn error(error: &AppError) {
    eprintln!("{} {}", paint(RED.bold(), "error:"), message(error));
}

pub fn note(text: impl std::fmt::Display) {
    eprintln!("{}", paint(DIM, text));
}

pub fn success(text: impl std::fmt::Display) {
    println!("{} {text}", paint(GREEN, "✓"));
}

/// Writes `bytes` to standard output as they are. A reader that stops early, like `head`, is
/// not an error.
pub fn raw(bytes: &[u8]) -> Result<(), AppError> {
    let mut stdout = io::stdout().lock();
    match stdout.write_all(bytes).and_then(|()| stdout.flush()) {
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => Err(AppError::io("standard output", e)),
        _ => Ok(()),
    }
}

pub fn json(value: &impl Serialize) {
    let text = serde_json::to_string_pretty(value).expect("output is plain data");
    println!("{}", terminal_safe_json(&text));
}

/// JSON escapes the ASCII control characters but leaves DEL, C1 controls and direction overrides
/// as they are; written as escapes they mean the same to a parser and nothing to a terminal.
fn terminal_safe_json(text: &str) -> String {
    use std::fmt::Write as _;

    let mut safe = String::with_capacity(text.len());
    for c in text.chars() {
        if (!c.is_ascii_control() && melcloud::unsafe_to_show(c)) || c == '\u{7f}' {
            let _ = write!(safe, "\\u{:04x}", u32::from(c));
        } else {
            safe.push(c);
        }
    }
    safe
}

fn fan_bars(fan: FanSpeed, levels: u8, style: Style) -> String {
    (1..=levels)
        .map(|level| {
            let lit = theme::fan_bar_lit(level, fan);
            paint(if lit { style } else { DIM }, theme::fan_bar(level, levels))
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
    let rows: Vec<_> = rows
        .into_iter()
        .map(|row| row.map(|cell| (cell, Style::new())))
        .collect();
    table(header, [false; 5], &rows);
    println!(
        "{}",
        paint(
            DIM,
            format!("updated {}", ago(fetched_at, Timestamp::now()))
        )
    );
}

/// A table with a dim header and columns as wide as their widest cell; `right` aligns numbers.
fn table<const N: usize>(header: [&str; N], right: [bool; N], rows: &[[(String, Style); N]]) {
    let widths: [usize; N] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].0.chars().count())
            .chain([header[column].len()])
            .max()
            .unwrap_or(0)
    });
    // Padding goes inside the color, and the last column gets none.
    let line = |cells: [(&str, Style); N]| {
        cells
            .into_iter()
            .zip(widths)
            .enumerate()
            .map(|(column, ((text, style), width))| {
                if right[column] {
                    paint(style, format!("{text:>width$}"))
                } else if column + 1 == N {
                    paint(style, text)
                } else {
                    paint(style, format!("{text:<width$}"))
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };
    println!("{}", line(header.map(|text| (text, DIM))));
    for row in rows {
        println!(
            "{}",
            line(row.each_ref().map(|(text, style)| (text.as_str(), *style)))
        );
    }
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
    fn json_escapes_what_a_terminal_would_act_on() {
        let value = serde_json::json!({"name": "a\u{1b}b\u{9b}c\u{202e}d\u{7f}é\n"});
        let text = terminal_safe_json(&serde_json::to_string(&value).unwrap());
        assert_eq!(text, r#"{"name":"a\u001bb\u009bc\u202ed\u007fé\n"}"#);
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back, value, "the same to a parser");
    }

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

/// Width of the terminal, or 80 columns when the output is not a terminal, so piped output does
/// not depend on the window.
fn columns() -> usize {
    if io::stdout().is_terminal() {
        ratatui::crossterm::terminal::size().map_or(80, |(width, _)| usize::from(width))
    } else {
        80
    }
}

/// Rows of the change table in `klima history`; `--json` has every sample.
const HISTORY_ROWS: usize = 20;
/// Width of the labels before the chart in `klima history`.
const HISTORY_LABELS: usize = 5;
/// The smallest temperature range the sparkline spreads over its height, in °C, so a tenth of a
/// degree does not look like a jump.
const SPARK_RANGE: f64 = 1.0;

/// `parts` joined by " · " on lines no wider than `width`, wrapping only between parts. The
/// first part, in bold, is shortened with "…" when it alone is too wide.
fn header(parts: &[String], width: usize) {
    let mut lines: Vec<String> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let shown = if index == 0 && part.chars().count() > width {
            let cut: String = part.chars().take(width.saturating_sub(1)).collect();
            paint(BOLD, format!("{cut}…"))
        } else if index == 0 {
            paint(BOLD, part)
        } else {
            paint(DIM, part)
        };
        let length = |line: &str| {
            anstream::adapter::strip_str(line)
                .to_string()
                .chars()
                .count()
        };
        match lines.last_mut() {
            Some(line) if length(line) + 3 + part.chars().count() <= width => {
                line.push_str(&paint(DIM, " · "));
                line.push_str(&shown);
            }
            _ => lines.push(shown),
        }
    }
    for line in lines {
        println!("{line}");
    }
}

/// `klima history`: the room temperature as a sparkline with the modes under it, and a table of
/// the changes of the settings. `samples` may start before `since`: the one standing when the
/// window opens is part of the picture. `span` describes the window.
pub fn history(device: &Device, span: &str, timeline: Timeline, samples: &[Sample], tz: &TimeZone) {
    let width = columns();
    let timeline = Timeline {
        width: width.saturating_sub(HISTORY_LABELS).clamp(10, 160),
        ..timeline
    };
    let since = timeline.since;
    let shown = samples.iter().filter(|s| s.at >= since).count();
    let count = match shown {
        1 => "1 sample".to_owned(),
        n => format!("{n} samples"),
    };
    let columns = timeline.columns(samples);
    let rooms = columns.iter().flatten().map(|c| c.room);
    let low = rooms.clone().fold(f64::INFINITY, f64::min);
    let high = rooms.fold(f64::NEG_INFINITY, f64::max);
    let mut parts = vec![device.name.clone(), format!("last {span}"), count];
    if shown > 0 {
        parts.push(if high - low < 0.05 {
            format!("room {low:.1} °C")
        } else {
            format!("room {low:.1} to {high:.1} °C")
        });
    }
    header(&parts, width);
    if shown == 0 {
        let latest = samples.iter().rev().find(|s| s.at < since);
        match latest {
            Some(latest) => note(format!(
                "No reads in the last {span}; the latest was {}.",
                ago(latest.at, timeline.now)
            )),
            None => note("Every read of the unit, by a command or the TUI, adds a sample."),
        }
        return;
    }
    history_chart(timeline, &columns, (low, high), tz);
    println!();
    let levels = device.capabilities.as_ref().map_or(0, |c| c.fan_levels);
    history_changes(timeline, samples, levels, tz);
}

/// The room temperature as a sparkline between `low` and `high`, the modes under it, the time
/// axis and a legend of the modes shown.
fn history_chart(
    timeline: Timeline,
    columns: &[Option<Column>],
    (low, high): (f64, f64),
    tz: &TimeZone,
) {
    let middle = f64::midpoint(low, high);
    let half = ((high - low) / 2.0).max(SPARK_RANGE / 2.0);
    let spark: String = columns
        .iter()
        .map(|column| match column {
            Some(c) => {
                let share = (c.room - (middle - half)) / (2.0 * half);
                paint(ROOM, theme::BARS[cell(share, theme::BARS.len() - 1)])
            }
            None => " ".to_owned(),
        })
        .collect();
    // Without colors, letters tell the modes apart.
    let colored = anstream::AutoStream::choice(&io::stdout()) != anstream::ColorChoice::Never;
    let mark = |mode: OperationMode| {
        if colored {
            paint(accent(mode), "▀")
        } else {
            mode.to_string().chars().next().unwrap_or('?').to_string()
        }
    };
    let modes: String = columns
        .iter()
        .map(|column| match column.map(|c| c.running) {
            Some(Some(mode)) => mark(mode),
            Some(None) => paint(DIM, "·"),
            None => " ".to_owned(),
        })
        .collect();
    let mut legend: Vec<String> = Vec::new();
    for mode in OperationMode::KNOWN {
        if columns.iter().flatten().any(|c| c.running == Some(mode)) {
            legend.push(format!("{} {}", mark(mode), paint(DIM, mode)));
        }
    }
    if columns.iter().flatten().any(|c| c.running.is_none()) {
        legend.push(paint(DIM, "· off"));
    }
    if columns.iter().any(Option::is_none) {
        legend.push(paint(DIM, "blank: not read"));
    }
    let labels = |text: &str| paint(DIM, format!("{text:<HISTORY_LABELS$}"));
    println!("{}{spark}", labels("room"));
    println!("{}{modes}", labels("mode"));
    println!("{}{}", labels(""), paint(DIM, timeline.axis(tz)));
    println!("{}{}", labels(""), legend.join("  "));
}

/// The state when the window opens, then each change of a setting, the latest ones.
fn history_changes(timeline: Timeline, samples: &[Sample], levels: u8, tz: &TimeZone) {
    let Timeline { since, .. } = timeline;
    let format = timeline.dates(tz).time();
    let time = |at: Timestamp| at.to_zoned(tz.clone()).strftime(format).to_string();
    // The sample standing when the window opens is shown at the start of the window.
    let standing = samples
        .iter()
        .rev()
        .find(|s| s.at < since)
        .filter(|s| since.duration_since(s.at) <= GAP);
    let first = standing.map(|s| (since, s));
    let inside = samples.iter().filter(|s| s.at >= since).map(|s| (s.at, s));
    let mut rows = Vec::new();
    let mut previous: Option<&Sample> = None;
    for (at, sample) in first.into_iter().chain(inside) {
        let changed = previous.is_none_or(|p| {
            p.power != sample.power
                || p.mode != sample.mode
                || p.fan != sample.fan
                || (p.target - sample.target).abs() > 0.01
        });
        let opening = previous.is_none();
        previous = Some(sample);
        if !changed {
            continue;
        }
        let tone = if sample.power {
            accent(sample.mode)
        } else {
            DIM
        };
        let plain = Style::new();
        let fan = match sample.fan {
            Some(FanSpeed::Level(level)) if levels > 0 => format!("{level}/{levels}"),
            Some(fan) => fan.to_string(),
            None => "?".to_owned(),
        };
        rows.push([
            (time(at), if opening { DIM } else { plain }),
            ((if sample.power { "on" } else { "off" }).to_owned(), plain),
            (sample.mode.to_string(), tone),
            (format!("{:.1}°", sample.target), plain),
            (fan, plain),
            (format!("{:.1}°", sample.room), plain),
        ]);
    }
    let earlier = rows.len().saturating_sub(HISTORY_ROWS);
    if earlier > 0 {
        let what = if earlier == 1 { "change" } else { "changes" };
        note(format!("{earlier} earlier {what} hidden; --json has all"));
    }
    table(
        ["FROM", "POWER", "MODE", "TARGET", "FAN", "ROOM"],
        [false, false, false, true, false, true],
        &rows[earlier..],
    );
}

#[derive(Serialize)]
struct HistoryJson<'a> {
    device: DeviceRef<'a>,
    since: Timestamp,
    samples: Vec<SampleJson>,
}

#[derive(Serialize)]
struct SampleJson {
    at: Timestamp,
    power: bool,
    mode: String,
    target_temperature: f64,
    room_temperature: f64,
    outside_temperature: Option<f64>,
    fan_speed: Option<String>,
}

pub fn history_json(device: &Device, since: Timestamp, samples: &[Sample]) {
    json(&HistoryJson {
        device: DeviceRef {
            id: device.id,
            name: &device.name,
        },
        since,
        samples: samples
            .iter()
            .filter(|s| s.at >= since)
            .map(|s| SampleJson {
                at: s.at,
                power: s.power,
                mode: s.mode.to_string(),
                target_temperature: s.target,
                room_temperature: s.room,
                outside_temperature: s.outside,
                fan_speed: s.fan.map(|fan| fan.to_string()),
            })
            .collect(),
    });
}
