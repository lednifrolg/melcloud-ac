//! Drawing pieces of the control pane: big digits and the temperature scale.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::chart::cell;

/// A 3×5 pixel font; every pixel is drawn two cells wide so it looks square.
const FONT: [(char, [&str; 5]); 11] = [
    ('0', ["###", "#.#", "#.#", "#.#", "###"]),
    ('1', [".#.", "##.", ".#.", ".#.", "###"]),
    ('2', ["###", "..#", "###", "#..", "###"]),
    ('3', ["###", "..#", ".##", "..#", "###"]),
    ('4', ["#.#", "#.#", "###", "..#", "..#"]),
    ('5', ["###", "#..", "###", "..#", "###"]),
    ('6', ["###", "#..", "###", "#.#", "###"]),
    ('7', ["###", "..#", "..#", "..#", "..#"]),
    ('8', ["###", "#.#", "###", "#.#", "###"]),
    ('9', ["###", "#.#", "###", "..#", "###"]),
    ('.', [".", ".", ".", ".", "#"]),
];

/// Height of [`BigText`] in rows.
pub const BIG_TEXT_HEIGHT: u16 = 5;

/// Digits and a decimal point, five rows tall.
pub struct BigText<'a> {
    pub text: &'a str,
    pub style: Style,
}

fn glyph(c: char) -> Option<&'static [&'static str; 5]> {
    FONT.iter().find(|(key, _)| *key == c).map(|(_, rows)| rows)
}

impl BigText<'_> {
    /// Columns needed: two per pixel and a two-column gap between glyphs.
    pub fn width(&self) -> u16 {
        let pixels: usize = self
            .text
            .chars()
            .filter_map(glyph)
            .map(|rows| rows[0].len())
            .sum();
        let gaps = self
            .text
            .chars()
            .filter(|&c| glyph(c).is_some())
            .count()
            .saturating_sub(1);
        u16::try_from(2 * (pixels + gaps)).unwrap_or(u16::MAX)
    }
}

impl Widget for BigText<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut x = area.x;
        for rows in self.text.chars().filter_map(glyph) {
            for (dy, row) in (0..BIG_TEXT_HEIGHT).zip(rows) {
                for (dx, pixel) in (0..).zip(row.chars()) {
                    let column = x + 2 * dx;
                    if pixel == '#' && column + 1 < area.right() && dy < area.height {
                        buf.set_string(column, area.y + dy, "██", self.style);
                    }
                }
            }
            x += 2 * u16::try_from(rows[0].len() + 1).unwrap_or(0);
        }
    }
}

/// A scale from `min` to `max` with the target (`●`) and the room temperature (`┃`) on it,
/// `width` cells wide without the labels.
pub fn scale(
    min: f64,
    max: f64,
    target: f64,
    room: f64,
    width: u16,
    on: Style,
    off: Style,
) -> Line<'static> {
    let cells = usize::from(width.max(2));
    let position = |value: f64| cell((value - min) / (max - min), cells - 1);
    let (target_at, room_at) = (position(target), position(room));
    let mut spans = vec![Span::styled(format!("{min:>2.0} "), off)];
    spans.extend((0..cells).map(|cell| {
        if cell == target_at {
            Span::styled("●", on.bold())
        } else if cell == room_at {
            Span::raw("┃")
        } else {
            Span::styled("━", if cell < target_at { on } else { off })
        }
    }));
    spans.push(Span::styled(format!(" {max:.0}"), off));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_text_is_two_cells_per_pixel() {
        let text = BigText {
            text: "22.5",
            style: Style::new(),
        };
        assert_eq!(text.width(), 6 + 2 + 6 + 2 + 2 + 2 + 6);
        let mut buf = Buffer::empty(Rect::new(0, 0, text.width(), BIG_TEXT_HEIGHT));
        text.render(buf.area, &mut buf);
        let rows: Vec<String> = (0..5)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect();
        assert_eq!(rows[0], "██████  ██████      ██████");
        assert_eq!(rows[4], "██████  ██████  ██  ██████");
    }

    #[test]
    fn scale_marks_target_and_room() {
        let line = scale(
            16.0,
            31.0,
            22.0,
            24.5,
            16,
            Style::new().cyan(),
            Style::new().dim(),
        );
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "16 ━━━━━━●━━┃━━━━━━ 31");
    }
}
