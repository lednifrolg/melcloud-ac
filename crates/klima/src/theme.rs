//! Colors and glyphs shared by the command line and the TUI, so both look the same.
//!
//! Colors are slots of the terminal's 16-color ANSI palette rather than fixed RGB values, so they
//! follow the terminal theme.

use melcloud::{FanSpeed, OperationMode, VaneHorizontal, VaneVertical};

/// Palette slot (0-15) that stands for a mode.
pub fn mode_color(mode: OperationMode) -> u8 {
    match mode {
        OperationMode::Heat => 11,
        OperationMode::Dry => 6,
        OperationMode::Cool => 14,
        OperationMode::Auto => 13,
        OperationMode::Fan | OperationMode::Other(_) => 7,
    }
}

/// Bars of rising height, for fan levels and sparklines.
pub const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Bar for fan level `level` of `levels`: rising from `▁` to `█`.
pub fn fan_bar(level: u8, levels: u8) -> char {
    let top = usize::from(levels.max(2)) - 1;
    BARS[(usize::from(level.max(1)) - 1) * 7 / top]
}

/// Whether the bar for `level` is lit at fan speed `fan`; every bar is lit in auto.
pub fn fan_bar_lit(level: u8, fan: FanSpeed) -> bool {
    match fan {
        FanSpeed::Auto => true,
        FanSpeed::Level(current) => level <= current,
        FanSpeed::Other(_) => false,
    }
}

/// Which of the five vane positions are marked: one for a position, both ends for split, none
/// for auto and swing.
fn marks(position: Option<u8>, split: bool) -> [bool; 5] {
    std::array::from_fn(|i| {
        let spot = u8::try_from(i + 1).unwrap_or_default();
        split && (spot == 1 || spot == 5) || position == Some(spot)
    })
}

pub fn vertical_marks(vane: VaneVertical) -> [bool; 5] {
    match vane {
        VaneVertical::Position(p) => marks(Some(p), false),
        _ => marks(None, false),
    }
}

pub fn horizontal_marks(vane: VaneHorizontal) -> [bool; 5] {
    match vane {
        VaneHorizontal::Position(p) => marks(Some(p), false),
        VaneHorizontal::Split => marks(None, true),
        _ => marks(None, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_bars_rise_over_the_levels() {
        let five: String = (1..=5).map(|level| fan_bar(level, 5)).collect();
        assert_eq!(five, "▁▂▄▆█");
        let four: String = (1..=4).map(|level| fan_bar(level, 4)).collect();
        assert_eq!(four, "▁▃▅█");
        assert!(fan_bar_lit(2, FanSpeed::Level(2)) && !fan_bar_lit(3, FanSpeed::Level(2)));
        assert!(fan_bar_lit(5, FanSpeed::Auto));
    }

    #[test]
    fn vane_marks_show_position_split_or_nothing() {
        assert_eq!(
            vertical_marks(VaneVertical::Position(3)),
            [false, false, true, false, false]
        );
        assert_eq!(
            horizontal_marks(VaneHorizontal::Split),
            [true, false, false, false, true]
        );
        assert_eq!(horizontal_marks(VaneHorizontal::Swing), [false; 5]);
    }
}
