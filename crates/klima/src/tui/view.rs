//! Drawing the TUI from the model.

use std::fmt::Display;

use jiff::civil::Date;
use jiff::civil::DateTime;
use jiff::{SignedDuration, Timestamp};
use melcloud::{
    AtaView, Capabilities, FanSpeed, VaneHorizontal, VaneVertical, WeatherKind, WeatherObservation,
};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, BorderType, Chart, Clear, Dataset, GraphType, Padding, Paragraph, Wrap,
};

use super::HISTORY_SPAN;
use super::keymap::{BINDINGS, Hint};
use super::model::{
    Busy, Flight, LoginForm, Model, NOTICE_SHOWN, Overlay, Phase, Quit, Row, TABS, Target, Tone,
    rows,
};
use super::widgets::{BIG_TEXT_HEIGHT, BigText, scale};
use crate::cache::Sample;
use crate::chart::{Column, GAP, Timeline};
use crate::control;
use crate::governor::HOURLY_LIMIT;
use crate::output::{ago, duration, shorten};
use crate::theme;

const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
/// Borders of panes without focus, and status line segments.
const MUTED: Color = Color::Indexed(8);
const GOOD: Color = Color::Indexed(2);
const WARN: Color = Color::Indexed(3);
const BAD: Color = Color::Indexed(1);
const OVERLAY: Color = Color::Indexed(4);
const BUSY: Color = Color::Indexed(6);
/// Background of the selected control row.
const SELECTED: Color = Color::Indexed(8);
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// The big digits for a moment after the target changed.
const FLASHED: Style = Style::new()
    .fg(Color::Indexed(15))
    .add_modifier(Modifier::BOLD);
/// A turning fan.
const FAN: [char; 6] = ['◜', '◠', '◝', '◞', '◡', '◟'];
/// Time a swinging vane takes from one position to the next.
const SWING_STEP: i64 = 250;

/// A screen area the mouse can point at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotspot {
    pub area: Rect,
    pub target: Target,
}

/// Smallest window for the narrow layout, and for the wide one.
const TABS_MIN: (u16, u16) = (46, 19);
const WIDE_MIN: (u16, u16) = (100, 28);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Wide,
    Tabs,
    TooSmall,
}

fn shape(area: Rect) -> Shape {
    let fits = |(width, height): (u16, u16)| area.width >= width && area.height >= height;
    if fits(WIDE_MIN) {
        Shape::Wide
    } else if fits(TABS_MIN) {
        Shape::Tabs
    } else {
        Shape::TooSmall
    }
}

/// Draws the whole screen and returns the areas the mouse can point at, most specific last.
pub fn render(model: &Model, frame: &mut Frame) -> Vec<Hotspot> {
    let area = frame.area();
    let shape = shape(area);
    let mut spots = Vec::new();
    if shape == Shape::TooSmall {
        too_small(frame, area);
        return spots;
    }
    let [body, status, hints] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    if shape == Shape::Wide {
        wide(model, frame, body, &mut spots);
    } else {
        tabs(model, frame, body, &mut spots);
    }
    statusline(model, frame, status);
    hint_bar(model, frame, hints, shape);
    match &model.overlay {
        Overlay::Help => help(frame, area),
        Overlay::Login(form) => login(model, form, frame, area),
        Overlay::None => return spots,
    }
    Vec::new()
}

fn wide(model: &Model, frame: &mut Frame, body: Rect, spots: &mut Vec<Hotspot>) {
    let [top, bottom] = Layout::vertical([Constraint::Fill(1), Constraint::Length(9)]).areas(body);
    let columns = || Layout::horizontal([Constraint::Fill(7), Constraint::Fill(4)]).spacing(1);
    let [control_area, side] = columns().areas(top);
    let [outside_area, unit_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(7)]).areas(side);
    let [history_area, activity_area] = columns().areas(bottom);
    control(model, frame, control_area, spots);
    outside(model, frame, outside_area);
    unit(model, frame, unit_area);
    history(model, frame, history_area);
    activity(model, frame, activity_area);
}

fn tabs(model: &Model, frame: &mut Frame, body: Rect, spots: &mut Vec<Hotspot>) {
    let [bar, pane] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(body);
    let spans = TABS.iter().enumerate().map(|(index, name)| {
        let label = format!(" {} {name} ", index + 1);
        if index == model.tab {
            Span::styled(label, BOLD.bg(MUTED))
        } else {
            Span::styled(label, DIM)
        }
    });
    frame.render_widget(Line::from(spans.collect::<Vec<_>>()), bar);
    match model.tab {
        0 => control(model, frame, pane, spots),
        1 => {
            let [outside_area, unit_area] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(7)]).areas(pane);
            outside(model, frame, outside_area);
            unit(model, frame, unit_area);
        }
        2 => history(model, frame, pane),
        _ => activity(model, frame, pane),
    }
}

fn pane<'a>(title: impl Into<Line<'a>>, border: Color) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .title(title)
        .padding(Padding::horizontal(1))
}

fn title(text: &str) -> Line<'_> {
    Line::from(vec![" ".into(), Span::styled(text, BOLD), " ".into()])
}

fn dim_title(text: &str) -> Line<'static> {
    Line::from(Span::styled(format!(" {text} "), DIM)).right_aligned()
}

fn mode_color(view: &AtaView) -> Option<Color> {
    view.power
        .then(|| Color::Indexed(theme::mode_color(view.mode)))
}

fn current_weather(view: &AtaView) -> Option<&WeatherObservation> {
    view.weather.iter().find(|w| w.kind == WeatherKind::Current)
}

fn control(model: &Model, frame: &mut Frame, area: Rect, spots: &mut Vec<Hotspot>) {
    let device = model.device();
    let name = device.map_or("klima", |d| d.name.as_str());
    let Some(view) = model.shown() else {
        let text = if model.account.is_none() {
            "Log in to see the unit."
        } else if model.busy.is_some() {
            "Reading the unit…"
        } else {
            "No state yet."
        };
        let block = pane(title(name), MUTED);
        frame.render_widget(Paragraph::new(text).style(DIM).block(block), area);
        return;
    };
    let color = mode_color(&view);
    let tone = color.map_or(DIM, |c| Style::new().fg(c));
    let power = if view.power {
        Span::styled("● on", Style::new().fg(GOOD))
    } else {
        Span::styled("○ off", DIM)
    };
    // A long name is cut, so the power and the model still fit on the border.
    let model_name = device.and_then(|d| d.model.clone());
    let room = usize::from(area.width)
        .saturating_sub(4 + power.width() + 2)
        .saturating_sub(model_name.as_ref().map_or(0, |m| m.chars().count() + 3));
    let heading = Line::from(vec![
        " ".into(),
        Span::styled(
            shorten(name, room.max(1)),
            tone.add_modifier(Modifier::BOLD),
        ),
        " ".into(),
        power,
        " ".into(),
    ]);
    let mut block = pane(heading, color.unwrap_or(MUTED));
    if let Some(model_name) = model_name {
        block = block.title_top(dim_title(&model_name));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The settings come before the command status, and both before the spacing: a setting that
    // can be selected must be on screen.
    let settings = device
        .and_then(|d| d.capabilities.as_ref())
        .map_or(0, |caps| {
            u16::try_from(rows(caps).len()).unwrap_or(u16::MAX)
        });
    let mut spare = inner.height.saturating_sub(BIG_TEXT_HEIGHT + 2);
    let mut take = |wanted: u16| {
        let given = wanted.min(spare);
        spare -= given;
        given
    };
    let settings = take(settings);
    let status = take(2);
    let gaps = [take(1), take(1)];
    let [digits_row, _, scale_rows, _, rows_area, status_area] = Layout::vertical([
        Constraint::Length(BIG_TEXT_HEIGHT),
        Constraint::Length(gaps[0]),
        Constraint::Length(2),
        Constraint::Length(gaps[1]),
        Constraint::Min(settings),
        Constraint::Length(status),
    ])
    .areas(inner);
    let flashing = model.flash_until.is_some_and(|until| until > model.now);
    let digits_style = if flashing { FLASHED } else { tone };
    temperature(&view, tone, digits_style, frame, digits_row, spots);

    let Some(caps) = device.and_then(|d| d.capabilities.as_ref()) else {
        return;
    };
    let range = caps.temperature_range(view.mode);
    let [scale_line, legend_line] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(scale_rows);
    let width = scale_line.width.saturating_sub(6);
    let line = scale(
        range.min,
        range.max,
        view.target_temperature,
        view.room_temperature,
        width,
        tone,
        DIM,
    );
    frame.render_widget(line, scale_line);
    let parts = [
        "   ● target  ┃ room".to_owned(),
        format!("{} {}-{}", view.mode, range.min, range.max),
        format!("step {}", caps.temperature_step),
    ];
    let legend = joined(parts, usize::from(legend_line.width));
    frame.render_widget(Line::styled(legend, DIM), legend_line);
    control_rows(model, &view, caps, color, frame, rows_area, spots);
    command_status(model, frame, status_area);
}

/// `parts` joined by " · " as far as they fit in `width`; the parts that do not fit are left
/// out from the end, never cut.
fn joined(parts: impl IntoIterator<Item = String>, width: usize) -> String {
    let mut text = String::new();
    for part in parts {
        let next = if text.is_empty() {
            part
        } else {
            format!("{text} · {part}")
        };
        if next.chars().count() > width {
            break;
        }
        text = next;
    }
    text
}

/// The target in big digits, with the room and outside temperature beside it.
fn temperature(
    view: &AtaView,
    tone: Style,
    digits_style: Style,
    frame: &mut Frame,
    area: Rect,
    spots: &mut Vec<Hotspot>,
) {
    let target = format!("{:.1}", view.target_temperature);
    let digits = BigText {
        text: &target,
        style: digits_style,
    };
    let [digits_area, _, side_area] = Layout::horizontal([
        Constraint::Length(digits.width()),
        Constraint::Length(3),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(digits, digits_area);
    spots.push(Hotspot {
        area: digits_area,
        target: Target::Row(Row::Temperature),
    });
    let mut side = vec![
        Line::from(vec![
            Span::styled("°C", tone.add_modifier(Modifier::BOLD)),
            Span::styled(" target", DIM),
        ]),
        Line::default(),
        Line::from(vec![
            format!("{:.1}°", view.room_temperature).into(),
            Span::styled(" room", DIM),
        ]),
    ];
    if let Some(weather) = current_weather(view) {
        side.push(Line::from(vec![
            format!("{:.0}°", weather.temperature).into(),
            Span::styled(" outside", DIM),
        ]));
    }
    frame.render_widget(Paragraph::new(side), side_area);
}

/// Width of the marker of the selected row and of the setting names.
const MARKER: u16 = 2;
const LABEL: u16 = 8;

/// A setting's name, as wide as the others.
fn label(text: &str) -> Span<'static> {
    Span::styled(format!("{text:<width$}", width = usize::from(LABEL)), DIM)
}

/// A vane row: the five positions, the marked ones in `tone`, and the setting.
fn vane(name: &str, marks: [bool; 5], setting: impl Display, tone: Style) -> Vec<Span<'static>> {
    let mut spans = vec![label(name)];
    spans.extend(marks.map(|on| {
        if on {
            Span::styled("●", tone)
        } else {
            Span::styled("∙", DIM)
        }
    }));
    spans.push(format!(" {setting}").into());
    spans
}

fn control_rows(
    model: &Model,
    view: &AtaView,
    caps: &Capabilities,
    color: Option<Color>,
    frame: &mut Frame,
    area: Rect,
    spots: &mut Vec<Hotspot>,
) {
    let selected = model.row;
    let moving = model.moving();
    // When not every setting fits, the list scrolls so the selected one shows.
    let all = rows(caps);
    let index = all.iter().position(|&row| row == selected).unwrap_or(0);
    let first = index.saturating_sub(usize::from(area.height).saturating_sub(1));
    let tone = color.map_or(DIM, |c| Style::new().fg(c));
    let mut specific = Vec::new();
    for (row, y) in all.into_iter().skip(first).zip(area.y..area.bottom()) {
        let x = area.x + MARKER + LABEL;
        let spans = match row {
            Row::Temperature => vec![
                label("temp"),
                Span::styled(format!("{:.1} °C", view.target_temperature), BOLD),
            ],
            Row::Mode => {
                // Pills start one column early so their names line up with the other values.
                let mut spans = vec![label("mode").content(format!("{:<7}", "mode"))];
                let mut pill_x = x - 1;
                for &mode in &caps.modes {
                    let pill = format!(" {mode} ");
                    let width = u16::try_from(pill.chars().count()).unwrap_or(0);
                    specific.push(Hotspot {
                        area: Rect::new(pill_x, y, width, 1),
                        target: Target::Mode(mode),
                    });
                    pill_x += width;
                    spans.push(match (mode == view.mode, color) {
                        (true, Some(color)) => Span::styled(pill, BOLD.fg(Color::Black).bg(color)),
                        (true, None) => Span::styled(pill, BOLD.add_modifier(Modifier::REVERSED)),
                        (false, _) => Span::styled(pill, DIM),
                    });
                }
                spans
            }
            Row::Fan => {
                let mut spans = if moving {
                    let glyph = fan_glyph(view.fan_speed, caps.fan_levels, model.now);
                    // The turning fan fits in the label, so the bars stay in line with the rest.
                    vec![
                        Span::styled("fan ", DIM),
                        Span::styled(glyph.to_string(), tone),
                        "   ".into(),
                    ]
                } else {
                    vec![label("fan")]
                };
                for (level, bar_x) in (1..=caps.fan_levels).zip(x..) {
                    let lit = theme::fan_bar_lit(level, view.fan_speed);
                    let bar = theme::fan_bar(level, caps.fan_levels).to_string();
                    spans.push(Span::styled(bar, if lit { tone } else { DIM }));
                    specific.push(Hotspot {
                        area: Rect::new(bar_x, y, 1, 1),
                        target: Target::Fan(melcloud::FanSpeed::Level(level)),
                    });
                }
                spans.push(match view.fan_speed {
                    melcloud::FanSpeed::Level(level) => {
                        format!(" {level}/{}", caps.fan_levels).into()
                    }
                    other => format!(" {other}").into(),
                });
                spans
            }
            Row::VaneVertical => {
                let marks = if moving && view.vane_vertical == VaneVertical::Swing {
                    swing(model.now, 0)
                } else {
                    theme::vertical_marks(view.vane_vertical)
                };
                vane("vane ↕", marks, view.vane_vertical, tone)
            }
            Row::VaneHorizontal => {
                // Half a sweep behind the vertical vane, so the two do not move in step.
                let marks = if moving && view.vane_horizontal == VaneHorizontal::Swing {
                    swing(model.now, 4)
                } else {
                    theme::horizontal_marks(view.vane_horizontal)
                };
                vane("vane ↔", marks, view.vane_horizontal, tone)
            }
        };
        let line_area = Rect::new(area.x, y, area.width, 1);
        spots.push(Hotspot {
            area: line_area,
            target: Target::Row(row),
        });
        let mut line = Line::from(
            [
                vec![if row == selected {
                    Span::styled("▸ ", tone)
                } else {
                    "  ".into()
                }],
                spans,
            ]
            .concat(),
        );
        if row == selected {
            line = line.style(Style::new().bg(SELECTED));
        }
        frame.render_widget(line, line_area);
    }
    spots.extend(specific);
}

/// The fan glyph at `now`: turning faster at higher speeds, at a middle speed in auto.
fn fan_glyph(fan: FanSpeed, levels: u8, now: Timestamp) -> char {
    let level = match fan {
        FanSpeed::Level(level) => level.min(levels),
        FanSpeed::Auto | FanSpeed::Other(_) => levels.div_ceil(2),
    };
    // One step per redraw at the top speed, five redraws at the lowest.
    let step = 125 * i64::from(levels + 1 - level.max(1));
    FAN[index(now.as_millisecond() / step, FAN.len())]
}

/// The position lit on a swinging vane at `now`: sweeping 1 to 5 and back, `offset` steps along.
fn swing(now: Timestamp, offset: i64) -> [bool; 5] {
    const SWEEP: [usize; 8] = [0, 1, 2, 3, 4, 3, 2, 1];
    let lit = SWEEP[index(now.as_millisecond() / SWING_STEP + offset, SWEEP.len())];
    std::array::from_fn(|i| i == lit)
}

/// `step` wrapped into `0..len`.
fn index(step: i64, len: usize) -> usize {
    let len = i64::try_from(len).unwrap_or(1);
    usize::try_from(step.rem_euclid(len)).unwrap_or(0)
}

/// The two lines under the settings: waiting changes, a command on its way, or how the last one
/// ended.
fn command_status(model: &Model, frame: &mut Frame, area: Rect) {
    let base = model.state.as_ref().map(|s| s.value.view().clone());
    let described = |changes| {
        base.as_ref()
            .map(|base| {
                control::describe(&changes, base)
                    .into_iter()
                    .map(|c| format!("{} {} → {}", c.field, c.from, c.to))
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
            .unwrap_or_default()
    };
    let spinner = || {
        let frame = model.now.as_millisecond().rem_euclid(800) / 80;
        Span::styled(
            SPINNER[usize::try_from(frame).unwrap_or(0)].to_string(),
            Style::new().fg(BUSY),
        )
    };
    let lines: Vec<Line> = if let Some(pending) = model.pending {
        let left = pending
            .deadline
            .duration_since(model.now)
            .max(SignedDuration::ZERO);
        let millis = |d: SignedDuration| u128::try_from(d.as_millis()).unwrap_or(0);
        let window = millis(super::model::MERGE_WINDOW).max(1);
        let filled = usize::try_from((millis(left) * 10).div_ceil(window))
            .unwrap_or(10)
            .min(10);
        let headline = if model.quit == Quit::Asking {
            Line::from(vec![
                Span::styled("QUIT? ", BOLD.fg(WARN)),
                Span::styled("these changes are not sent yet", Style::new().fg(WARN)),
            ])
        } else {
            Line::from(vec![
                Span::styled("PENDING ", BOLD.fg(WARN)),
                Span::styled("━".repeat(filled), Style::new().fg(WARN)),
                Span::styled("─".repeat(10 - filled), DIM),
                Span::styled(
                    format!(" {:.1} s", left.as_secs_f64()),
                    Style::new().fg(WARN),
                ),
            ])
        };
        vec![headline, Line::raw(described(pending.changes))]
    } else if let Some(Flight {
        changes,
        phase,
        since,
    }) = model.flight
    {
        let doing = match phase {
            Phase::Sending => "sending".to_owned(),
            Phase::Confirming => {
                let seconds = model.now.duration_since(since).as_secs_f64();
                format!("waiting for the adapter · {seconds:.1} s")
            }
        };
        vec![
            Line::from(vec![
                spinner(),
                " ".into(),
                Span::styled(doing, Style::new().fg(BUSY)),
            ]),
            Line::styled(described(changes), DIM),
        ]
    } else if let Some(notice) = model
        .notice
        .as_ref()
        .filter(|n| n.at + NOTICE_SHOWN > model.now)
    {
        let (mark, color) = match notice.tone {
            Tone::Good => ("✓", GOOD),
            Tone::Info => ("·", Color::Reset),
            Tone::Warn => ("!", WARN),
            Tone::Bad => ("✗", BAD),
        };
        vec![Line::from(vec![
            Span::styled(mark, Style::new().fg(color)),
            " ".into(),
            Span::styled(notice.text.clone(), Style::new().fg(color)),
        ])]
    } else {
        Vec::new()
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

/// "now", "today", "tonight", "tomorrow", "Wed night", ... for a forecast entry at `time`.
fn weather_label(kind: WeatherKind, time: DateTime, today: Date) -> String {
    let date = time.date();
    let tomorrow = today.tomorrow().ok();
    let weekday = |date: Date| date.strftime("%a").to_string();
    match kind {
        WeatherKind::Current => "now".into(),
        WeatherKind::Day if date == today => "today".into(),
        WeatherKind::Day if Some(date) == tomorrow => "tomorrow".into(),
        WeatherKind::Day => weekday(date),
        WeatherKind::Night => {
            // A night entry dated after midnight belongs to the evening before.
            let night = if time.hour() < 12 {
                date.yesterday().unwrap_or(date)
            } else {
                date
            };
            if night == today {
                "tonight".into()
            } else if Some(night) == tomorrow {
                "tmrw night".into()
            } else {
                format!("{} night", weekday(night))
            }
        }
        WeatherKind::Other(_) => time.strftime("%a %H:%M").to_string(),
    }
}

/// "Clear/Sunny" reads as "clear".
fn condition(name: &str) -> String {
    name.split('/').next().unwrap_or(name).trim().to_lowercase()
}

fn outside(model: &Model, frame: &mut Frame, area: Rect) {
    let block = pane(title("Outside"), MUTED).title_top(dim_title("MELCloud weather"));
    let weather = model
        .state
        .as_ref()
        .map(|s| s.value.view().weather.as_slice())
        .unwrap_or_default();
    let today = model.now.to_zoned(model.tz.clone()).date();
    let mut lines: Vec<Line> = weather
        .iter()
        .map(|w| {
            Line::from(vec![
                Span::styled(format!("{:<11}", weather_label(w.kind, w.time, today)), DIM),
                Span::styled(format!("{:>3.0}°", w.temperature), BOLD),
                format!("  {:<11}", condition(&w.condition)).into(),
                Span::styled(format!("{:>3.0}%", w.humidity), DIM),
            ])
        })
        .collect();
    if let Some(sun) = weather
        .iter()
        .find(|w| w.sunrise.is_some() && w.sunset.is_some())
    {
        let time = |t: Option<DateTime>| {
            t.map(|t| t.strftime("%H:%M").to_string())
                .unwrap_or_default()
        };
        lines.push(Line::styled(
            format!(
                "sunrise {} · sunset {}",
                time(sun.sunrise),
                time(sun.sunset)
            ),
            DIM,
        ));
    }
    if lines.is_empty() {
        lines.push(Line::styled("no weather data", DIM));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn wifi_bars(dbm: i32) -> Vec<Span<'static>> {
    let strength = match dbm {
        -55.. => 4,
        -65.. => 3,
        -75.. => 2,
        _ => 1,
    };
    ["▂", "▄", "▆", "█"]
        .iter()
        .zip(1..)
        .map(|(bar, level)| {
            Span::styled(
                *bar,
                if level <= strength {
                    Style::new().fg(GOOD)
                } else {
                    DIM
                },
            )
        })
        .collect()
}

fn unit(model: &Model, frame: &mut Frame, area: Rect) {
    let device = model.device();
    let view = model.state.as_ref().map(|s| s.value.view());
    let label = |text: &str| Span::styled(format!("{text:<9}"), DIM);
    let mut lines = Vec::new();
    if let Some(dbm) = device.and_then(|d| d.wifi_signal_dbm) {
        let mut spans = vec![label("wifi")];
        spans.extend(wifi_bars(dbm));
        spans.push(format!(" {dbm} dBm").into());
        lines.push(Line::from(spans));
    }
    if let Some(meter) = device.and_then(|d| d.energy_meter) {
        lines.push(Line::from(vec![
            label("energy"),
            format!("{}.{} kWh", meter / 1000, meter % 1000 / 100).into(),
            Span::styled(" total ?", DIM),
        ]));
    }
    if let Some(last) = view.and_then(|v| v.last_communication) {
        let mut spans = vec![label("contact"), ago(last, model.now).into()];
        if let Some(next) = view.and_then(|v| v.next_communication) {
            let seconds = next.duration_since(model.now).as_secs();
            let when = if seconds > 0 {
                format!(" · next in {seconds} s")
            } else {
                " · next due".into()
            };
            spans.push(Span::styled(when, DIM));
        }
        lines.push(Line::from(spans));
    }
    if let Some(device) = device {
        let link = if device.instant_delivery {
            vec![
                Span::styled("MQTT", Style::new().fg(Color::Indexed(6))),
                Span::styled(" · instant delivery", DIM),
            ]
        } else {
            vec![
                "cloud polling".into(),
                Span::styled(" · applies at next contact", DIM),
            ]
        };
        lines.push(Line::from([vec![label("link")], link].concat()));
    }
    if let Some(view) = view {
        let errors = if view.has_error() {
            let message = view.error_message.clone().unwrap_or_default();
            Span::styled(
                format!("{} {message}", view.error_code),
                Style::new().fg(BAD),
            )
        } else {
            Span::styled("none", Style::new().fg(GOOD))
        };
        lines.push(Line::from(vec![label("errors"), errors]));
    }
    frame.render_widget(
        Paragraph::new(lines).block(pane(title("Unit"), MUTED)),
        area,
    );
}

/// Colors of the room and target temperature in the history chart.
const ROOM: Color = Color::Indexed(4);
const TARGET: Color = MUTED;

fn history(model: &Model, frame: &mut Frame, area: Rect) {
    let since = model.now - HISTORY_SPAN;
    let samples: Vec<Sample> = model
        .history
        .iter()
        .filter(|s| s.at >= since)
        .copied()
        .collect();
    let mut block = pane(title("History · 24h"), MUTED);
    match samples.len() {
        0 => {}
        1 => block = block.title_top(dim_title("1 sample")),
        n => block = block.title_top(dim_title(&format!("{n} samples"))),
    }
    if samples.is_empty() {
        let text = "Every read of the unit adds a sample of the room temperature; the chart fills up over the day.";
        frame.render_widget(
            Paragraph::new(text)
                .style(DIM)
                .wrap(Wrap { trim: true })
                .block(block),
            area,
        );
        return;
    }
    // The legend sits on the border, away from the values on the left that it could be read as.
    // Without colors, the target is drawn in dots instead of a line.
    let (target_marker, target_mark) = if model.colors {
        (Marker::Braille, "━")
    } else {
        (Marker::Dot, "•")
    };
    let mut legend = vec![" ".into(), Span::styled("━ room", Style::new().fg(ROOM))];
    if samples.iter().any(|s| s.power) {
        legend.extend([
            "  ".into(),
            Span::styled(format!("{target_mark} target"), Style::new().fg(TARGET)),
        ]);
    }
    legend.push(" ".into());
    block = block.title_bottom(Line::from(legend).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [plot_rows, ribbon_row, axis_row] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let columns = Layout::horizontal([Constraint::Length(7), Constraint::Fill(1)]);
    let [labels, plot] = columns.areas(plot_rows);
    let [ribbon_label, ribbon] = columns.areas(ribbon_row);
    let [_, axis] = columns.areas(axis_row);
    let timeline = Timeline {
        since,
        now: model.now,
        width: usize::from(plot.width),
    };

    // The target matters only while the unit runs.
    let target = |s: &Sample| s.power.then_some(s.target);
    let values: Vec<f64> = samples
        .iter()
        .flat_map(|s| [Some(s.room), target(s)])
        .flatten()
        .collect();
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut label_lines = vec![Line::default(); usize::from(labels.height)];
    if high - low < 0.05 {
        // A flat line runs mid-height, with its one value beside it.
        let middle = label_lines.len() / 2;
        if let Some(line) = label_lines.get_mut(middle) {
            *line = Line::styled(format!("{high:.1}°"), DIM);
        }
    } else if let [first, .., last] = label_lines.as_mut_slice() {
        *first = Line::styled(format!("{high:.1}°"), DIM);
        *last = Line::styled(format!("{low:.1}°"), DIM);
    }
    frame.render_widget(Paragraph::new(label_lines), labels);
    frame.render_widget(Line::styled("mode", DIM), ribbon_label);

    let targets = segments(&samples, &timeline, target);
    let rooms = segments(&samples, &timeline, |s| Some(s.room));
    // The room line is drawn last, so it stays on top where the two meet.
    let lines = targets
        .iter()
        .map(|points| (points, TARGET, target_marker))
        .chain(rooms.iter().map(|points| (points, ROOM, Marker::Braille)));
    let datasets = lines
        .map(|(points, color, marker)| {
            Dataset::default()
                .marker(marker)
                .graph_type(GraphType::Line)
                .style(Style::new().fg(color))
                .data(points)
        })
        .collect();
    // About a braille dot above and below, so the extremes land on the rows of their labels and a
    // flat day runs mid-height.
    let margin = ((high - low) / (f64::from(plot.height) * 4.0)).max(0.05);
    let chart = Chart::new(datasets)
        .x_axis(Axis::default().bounds([0.0, 1.0]))
        .y_axis(Axis::default().bounds([low - margin, high + margin]))
        .legend_position(None);
    frame.render_widget(chart, plot);
    frame.render_widget(mode_ribbon(&timeline, &model.history, model.colors), ribbon);
    frame.render_widget(Line::styled(timeline.axis(&model.tz), DIM), axis);
}

/// Points of `value` over the timeline, split where the unit went unread for longer than
/// [`GAP`] or `value` has nothing.
fn segments(
    samples: &[Sample],
    timeline: &Timeline,
    value: impl Fn(&Sample) -> Option<f64>,
) -> Vec<Vec<(f64, f64)>> {
    let mut segments: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut previous: Option<Timestamp> = None;
    for sample in samples {
        let Some(y) = value(sample) else {
            previous = None;
            continue;
        };
        let point = (timeline.share(sample.at), y);
        match segments.last_mut() {
            Some(segment) if previous.is_some_and(|p| sample.at.duration_since(p) <= GAP) => {
                segment.push(point);
            }
            _ => segments.push(vec![point]),
        }
        previous = Some(sample.at);
    }
    segments
}

/// One cell per column of the chart: the mode the unit ran in, a dot while it was off, nothing
/// while it was not read. Without colors, the mode's first letter.
fn mode_ribbon(timeline: &Timeline, samples: &[Sample], colors: bool) -> Line<'static> {
    let cells = timeline
        .columns(samples)
        .into_iter()
        .map(|column| match column {
            Some(Column {
                running: Some(mode),
                ..
            }) if !colors => mode.to_string().chars().take(1).collect::<String>().into(),
            Some(Column {
                running: Some(mode),
                ..
            }) => Span::styled(
                "▀",
                Style::new().fg(Color::Indexed(theme::mode_color(mode))),
            ),
            Some(_) => Span::styled("·", DIM),
            None => " ".into(),
        });
    Line::from(cells.collect::<Vec<_>>())
}

/// Splits `text` into lines of at most `width` characters at spaces; a longer word, like a URL,
/// is broken where the line ends.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let chars = |word: &str| word.chars().collect::<Vec<_>>();
    let pieces = text.split_whitespace().flat_map(|word| {
        chars(word)
            .chunks(width)
            .map(|piece| piece.iter().collect::<String>())
            .collect::<Vec<_>>()
    });
    for word in pieces {
        let word = word.as_str();
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// Width of the time and mark before each activity entry: `00:00:00 ✓ `.
const ACTIVITY_PREFIX: usize = 11;

fn activity(model: &Model, frame: &mut Frame, area: Rect) {
    let block = pane(title("Activity"), MUTED);
    let width = usize::from(block.inner(area).width)
        .saturating_sub(ACTIVITY_PREFIX)
        .max(12);
    let mut lines: Vec<Line> = Vec::new();
    for entry in &model.activity {
        let (mark, style) = match entry.tone {
            Tone::Good => ("✓", Style::new().fg(GOOD)),
            Tone::Info => ("·", DIM),
            Tone::Warn => ("!", Style::new().fg(WARN)),
            Tone::Bad => ("✗", Style::new().fg(BAD)),
        };
        let time = entry
            .at
            .to_zoned(model.tz.clone())
            .strftime("%H:%M:%S")
            .to_string();
        for (index, text) in wrap_words(&entry.text, width).into_iter().enumerate() {
            lines.push(if index == 0 {
                Line::from(vec![
                    Span::styled(time.clone(), DIM),
                    " ".into(),
                    Span::styled(mark, style),
                    " ".into(),
                    text.into(),
                ])
            } else {
                Line::from(vec![" ".repeat(ACTIVITY_PREFIX).into(), text.into()])
            });
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled("nothing yet", DIM));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// A device name in the status line is cut with "…" to this length before the link and the
/// countdown give way, and to the shorter one after.
const SHORT_NAME: usize = 20;
const SHORTEST_NAME: usize = 12;

fn statusline(model: &Model, frame: &mut Frame, area: Rect) {
    let view = model.state.as_ref().map(|s| s.value.view());
    let color = view.and_then(mode_color).unwrap_or(Color::Indexed(7));
    let label = match (model.busy, &model.overlay) {
        _ if model.quit == Quit::Asking => "QUIT?",
        _ if model.pending.is_some() => "PENDING",
        _ if model.flight.is_some() => "SENDING",
        (Some(Busy::Login), _) | (_, Overlay::Login(_)) => "LOGIN",
        (Some(_), _) => "LOADING",
        (None, _) => "READY",
    };
    let mut name = model
        .device()
        .map_or("klima", |d| d.name.as_str())
        .to_owned();
    // Each segment has the rank it is dropped in when the window is narrow: highest first. Rank
    // 0 stays longest, after a long device name has been shortened.
    let mut segments: Vec<(u8, Vec<Span>)> = Vec::new();
    if let Some(until) = model.paused_until.filter(|&until| until > model.now) {
        let until = until
            .to_zoned(model.tz.clone())
            .strftime("%H:%M")
            .to_string();
        let text = format!("○ requests paused until {until}");
        segments.push((0, vec![Span::styled(text, Style::new().fg(WARN))]));
    } else if model.unreachable {
        segments.push((
            0,
            vec![Span::styled("○ MELCloud unreachable", Style::new().fg(BAD))],
        ));
    } else if let Some(view) = view {
        let mut link = vec![if view.offline {
            Span::styled("○ offline", Style::new().fg(WARN))
        } else {
            Span::styled("● online", Style::new().fg(GOOD))
        }];
        if model.device().is_some_and(|d| d.instant_delivery) {
            link.push(Span::styled(" · MQTT", DIM));
        }
        segments.push((2, link));
    }
    segments.push((1, refresh_countdown(model)));
    let budget = Style::new().fg(match model.requests_last_hour {
        n if n >= HOURLY_LIMIT => BAD,
        n if n >= HOURLY_LIMIT * 4 / 5 => WARN,
        _ => Color::Reset,
    });
    let api = format!("api {}/{HOURLY_LIMIT}·h", model.requests_last_hour);
    segments.push((3, vec![Span::styled(api, budget)]));
    let session = match &model.account {
        Some(account) => match account.expires_at {
            Some(until) => Span::raw(format!(
                "session {}",
                duration(until.duration_since(model.now))
            )),
            None => Span::raw("logged in"),
        },
        None => Span::styled("not logged in", Style::new().fg(BAD)),
    };
    segments.push((4, vec![session]));

    let line = |name: &str, segments: &[(u8, Vec<Span<'static>>)]| {
        let mut spans = vec![
            Span::styled(format!(" {label} "), BOLD.fg(Color::Black).bg(color)),
            Span::styled(format!(" {name} "), BOLD.bg(MUTED)),
        ];
        for (index, (_, segment)) in segments.iter().enumerate() {
            spans.push(if index == 0 {
                " ".into()
            } else {
                Span::styled(" │ ", DIM)
            });
            spans.extend(segment.iter().cloned());
        }
        Line::from(spans)
    };
    // Narrow windows lose the budget and the session first, then a long name gets shorter, then
    // the link and the countdown go, then the name gets shorter still. What rank 0 says stays.
    let width = usize::from(area.width);
    loop {
        let over = line(&name, &segments).width().saturating_sub(width);
        if over == 0 {
            break;
        }
        let least = (0..segments.len()).max_by_key(|&i| segments[i].0);
        let rank = least.map_or(0, |i| segments[i].0);
        let length = name.chars().count();
        let shortest = if rank > 2 {
            usize::MAX
        } else if rank > 0 {
            SHORT_NAME
        } else {
            SHORTEST_NAME
        };
        match least {
            _ if length > shortest => {
                name = shorten(&name, length.saturating_sub(over).max(shortest));
            }
            Some(i) => drop(segments.remove(i)),
            None => break,
        }
    }
    frame.render_widget(line(&name, &segments), area);
}

fn refresh_countdown(model: &Model) -> Vec<Span<'static>> {
    if model.busy == Some(Busy::State) {
        return vec![Span::raw("↻ refreshing")];
    }
    let Some(at) = model.next_refresh else {
        return vec![Span::styled("↻ -", DIM)];
    };
    let left = at.duration_since(model.now).as_secs().max(0);
    let every = model.refresh_every.as_secs().max(1);
    let filled = usize::try_from((left * 5 + every / 2) / every)
        .unwrap_or(0)
        .min(5);
    vec![
        Span::raw(format!("↻ {}:{:02} ", left / 60, left % 60)),
        Span::raw("━".repeat(filled)),
        Span::styled("─".repeat(5 - filled), DIM),
    ]
}

fn hint_bar(model: &Model, frame: &mut Frame, area: Rect, shape: Shape) {
    let controllable = model.account.is_some()
        && model.shown().is_some()
        && model.device().is_some_and(|d| d.capabilities.is_some());
    let shown = |hint: Hint| match hint {
        Hint::Always => true,
        Hint::Control => controllable,
        Hint::Pending => model.pending.is_some(),
        Hint::Tabs => shape == Shape::Tabs,
        Hint::Never => false,
    };
    // (rank, keys, label); a lower rank stays longer when the window is narrow.
    let mut hints: Vec<(u8, &str, &str)> = match &model.overlay {
        Overlay::Login(_) => vec![
            (0, "⏎", "log in"),
            (0, "Tab", "next field"),
            (0, "Esc", "cancel"),
        ],
        Overlay::Help => vec![(0, "?", "close")],
        Overlay::None if model.quit == Quit::Asking => {
            vec![
                (0, "⏎", "send and quit"),
                (0, "u", "discard and quit"),
                (0, "Esc", "stay"),
            ]
        }
        Overlay::None => {
            let refresh = if model.account.is_some() {
                "refresh"
            } else {
                "log in"
            };
            let mut bindings: Vec<_> = BINDINGS.iter().filter(|b| shown(b.hint)).collect();
            // Waiting changes come first: they are what the next key is most likely for.
            bindings.sort_by_key(|b| b.hint != Hint::Pending);
            bindings
                .into_iter()
                .map(|b| {
                    (
                        b.rank,
                        b.short,
                        if b.short == "r" { refresh } else { b.label },
                    )
                })
                .collect()
        }
    };
    let line = |hints: &[(u8, &str, &str)]| {
        let mut spans = vec![Span::raw(" ")];
        for (_, key, label) in hints {
            spans.push(Span::styled(key.to_string(), BOLD));
            spans.push(Span::styled(format!(" {label}  "), DIM));
        }
        Line::from(spans)
    };
    while line(&hints).width() > usize::from(area.width) {
        let Some(least) = (0..hints.len()).max_by_key(|&i| hints[i].0) else {
            break;
        };
        hints.remove(least);
    }
    frame.render_widget(line(&hints), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [cell] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    cell
}

fn help(frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = BINDINGS
        .iter()
        .map(|b| {
            Line::from(vec![
                Span::styled(format!("{:<8}", b.keys), BOLD),
                b.help.into(),
            ])
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::styled("? or Esc closes this help", DIM));
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX) + 2;
    let popup = centered(area, 60.min(area.width), height.min(area.height));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(pane(title("Keys"), OVERLAY)),
        popup,
    );
}

fn login(model: &Model, form: &LoginForm, frame: &mut Frame, area: Rect) {
    let field = |name: &str, value: String, focused: bool| {
        let mut spans = vec![
            Span::styled(format!("{name:<10}"), DIM),
            Span::styled(value, BOLD),
        ];
        if focused {
            spans.push(Span::styled("▏", Style::new().fg(OVERLAY)));
        }
        Line::from(spans)
    };
    let status = if model.busy == Some(Busy::Login) {
        Line::raw("logging in…")
    } else if let Some(error) = &form.error {
        Line::styled(error.clone(), Style::new().fg(BAD))
    } else {
        Line::default()
    };
    let lines = vec![
        field("email", form.email.clone(), !form.on_password),
        field(
            "password",
            "•".repeat(form.password.chars().count()),
            form.on_password,
        ),
        Line::default(),
        status,
        Line::default(),
        Line::styled(
            "klima keeps only the session; the password is never stored.",
            DIM,
        ),
    ];
    let popup = centered(area, 66.min(area.width), 8.min(area.height));
    frame.render_widget(Clear, popup);
    let block = pane(title("MELCloud login"), OVERLAY);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(block),
        popup,
    );
}

fn too_small(frame: &mut Frame, area: Rect) {
    let (width, height) = TABS_MIN;
    let text = format!(
        "klima needs a window of at least {width}×{height} cells; this one is {}×{}.",
        area.width, area.height
    );
    let [middle] = Layout::vertical([Constraint::Length(3)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        middle,
    );
}

#[cfg(test)]
mod tests {
    use jiff::SignedDuration;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use melcloud::OperationMode;

    use super::*;
    use crate::tui::model::tests::{now, start, state};
    use crate::tui::model::{Msg, Start};
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    fn screen(model: &Model, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render(model, frame);
            })
            .unwrap();
        terminal.backend().to_string()
    }

    /// A day of samples and some activity, as after a while of use.
    fn lived_in() -> Model {
        // Off overnight, not read for a while, then heating and lately cooling.
        let history = (0..96)
            .filter(|quarters| !(30..40).contains(quarters))
            .map(|quarters: i32| {
                let wave = f64::from((quarters - 48).abs()) / 20.0;
                Sample {
                    at: now() - SignedDuration::from_mins(i64::from(15 * (96 - quarters))),
                    power: quarters >= 40,
                    mode: if quarters < 80 {
                        OperationMode::Heat
                    } else {
                        OperationMode::Cool
                    },
                    target: 22.0,
                    room: 21.0 + wave,
                    outside: Some(2.0),
                    fan: Some(FanSpeed::Level(2)),
                }
            })
            .collect();
        let (mut model, _) = Model::start(Start { history, ..start() });
        model.update(Msg::State {
            result: Ok(state(SignedDuration::from_mins(2))),
            manual: false,
        });
        model
    }

    #[test]
    fn wide_layout() {
        insta::assert_snapshot!(screen(&lived_in(), 120, 30));
    }

    #[test]
    fn narrow_layout_tabs() {
        let mut model = lived_in();
        for tab in 0..TABS.len() {
            model.tab = tab;
            insta::assert_snapshot!(format!("tab_{tab}"), screen(&model, 64, 22));
        }
    }

    #[test]
    fn login_form_over_the_last_known_state() {
        let (model, _) = Model::start(Start {
            account: None,
            email: Some("user@example.com".into()),
            ..start()
        });
        insta::assert_snapshot!(screen(&model, 120, 30));
    }

    #[test]
    fn help_overlay() {
        let mut model = lived_in();
        model.overlay = Overlay::Help;
        insta::assert_snapshot!(screen(&model, 120, 30));
    }

    #[test]
    fn waiting_changes_and_their_countdown() {
        let mut model = lived_in();
        model.update(Msg::Key(KeyCode::Char('+').into()));
        model.update(Msg::Key(KeyCode::Char('f').into()));
        model.update(Msg::Key(KeyCode::Char('j').into()));
        model.update(Msg::Tick(model.now + SignedDuration::from_millis(1200)));
        insta::assert_snapshot!(screen(&model, 120, 30));
    }

    #[test]
    fn command_on_its_way_and_its_result() {
        let mut model = lived_in();
        model.update(Msg::Key(KeyCode::Char('P').into()));
        model.update(Msg::Key(KeyCode::Enter.into()));
        model.update(Msg::Sent(Ok(())));
        model.update(Msg::Tick(model.now + SignedDuration::from_millis(1700)));
        insta::assert_snapshot!("confirming", screen(&model, 120, 30));
        model.update(Msg::Confirmed {
            result: Ok(Some(SignedDuration::from_millis(1100))),
            state: None,
        });
        insta::assert_snapshot!("applied", screen(&model, 120, 30));
    }

    #[test]
    fn quitting_with_waiting_changes() {
        let mut model = lived_in();
        model.update(Msg::Key(KeyCode::Char('m').into()));
        model.update(Msg::Key(KeyCode::Char('q').into()));
        insta::assert_snapshot!(screen(&model, 120, 30));
    }

    #[test]
    fn hotspots_point_at_modes_fan_levels_and_rows() {
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let mut spots = Vec::new();
        terminal
            .draw(|frame| spots = render(&lived_in(), frame))
            .unwrap();
        let at = |x, y| {
            spots
                .iter()
                .rev()
                .find(|s| s.area.contains(ratatui::layout::Position::new(x, y)))
                .map(|s| s.target)
        };
        // Rows 10 to 14 hold the settings; values start at column 12, mode pills at 11.
        assert_eq!(
            at(12, 11),
            Some(Target::Mode(melcloud::OperationMode::Heat))
        );
        assert_eq!(at(30, 11), Some(Target::Mode(melcloud::OperationMode::Fan)));
        assert_eq!(at(14, 12), Some(Target::Fan(melcloud::FanSpeed::Level(3))));
        assert_eq!(at(60, 13), Some(Target::Row(Row::VaneVertical)));
        assert_eq!(at(5, 3), Some(Target::Row(Row::Temperature)));
    }

    #[test]
    fn every_setting_is_on_screen_when_selected() {
        for (width, height, tab) in [(46, 19, 0), (100, 28, 0)] {
            let mut model = lived_in();
            model.tab = tab;
            for _ in 0..4 {
                model.update(Msg::Key(KeyEvent::from(KeyCode::Down)));
            }
            let text = screen(&model, width, height);
            assert!(text.contains("▸ vane ↔"), "{width}x{height}:\n{text}");
            for _ in 0..4 {
                model.update(Msg::Key(KeyEvent::from(KeyCode::Up)));
            }
            let text = screen(&model, width, height);
            assert!(text.contains("▸ temp"), "{width}x{height}:\n{text}");
        }
    }

    #[test]
    fn window_too_small() {
        insta::assert_snapshot!(screen(&lived_in(), 40, 12));
    }

    #[test]
    fn weather_labels_follow_the_day() {
        let today: Date = "2026-01-15".parse().unwrap();
        let label = |kind, time: &str| weather_label(kind, time.parse().unwrap(), today);
        assert_eq!(label(WeatherKind::Current, "2026-01-15T09:00"), "now");
        assert_eq!(label(WeatherKind::Day, "2026-01-15T12:00"), "today");
        assert_eq!(label(WeatherKind::Night, "2026-01-16T00:00"), "tonight");
        assert_eq!(label(WeatherKind::Day, "2026-01-16T12:00"), "tomorrow");
        assert_eq!(label(WeatherKind::Night, "2026-01-17T03:00"), "tmrw night");
        assert_eq!(label(WeatherKind::Day, "2026-01-17T12:00"), "Sat");
    }

    #[test]
    fn the_fan_turns_faster_at_higher_speeds() {
        let glyphs = |fan, every: i64| {
            (0..8)
                .map(|step| {
                    let at = Timestamp::from_millisecond(step * every).unwrap();
                    fan_glyph(fan, 5, at)
                })
                .collect::<String>()
        };
        assert_eq!(glyphs(FanSpeed::Level(5), 125), "◜◠◝◞◡◟◜◠");
        assert_eq!(glyphs(FanSpeed::Level(1), 625), "◜◠◝◞◡◟◜◠");
        assert_eq!(glyphs(FanSpeed::Level(1), 125), "◜◜◜◜◜◠◠◠");
        assert_eq!(glyphs(FanSpeed::Auto, 375), glyphs(FanSpeed::Level(3), 375));
    }

    #[test]
    fn swinging_vanes_sweep_and_come_back() {
        let lit = |step: i64, offset| {
            let at = Timestamp::from_millisecond(step * SWING_STEP).unwrap();
            swing(at, offset).iter().position(|&on| on).unwrap()
        };
        let sweep: Vec<usize> = (0..9).map(|step| lit(step, 0)).collect();
        assert_eq!(sweep, [0, 1, 2, 3, 4, 3, 2, 1, 0]);
        assert_eq!(lit(0, 4), 4, "half a sweep behind starts at the other end");
    }

    #[test]
    fn activity_wraps_under_the_text() {
        assert_eq!(
            wrap_words("the unit was read moments ago", 12),
            ["the unit was", "read moments", "ago"]
        );
        assert_eq!(
            wrap_words("a verylongword b", 4),
            ["a", "very", "long", "word", "b"]
        );
    }
}
