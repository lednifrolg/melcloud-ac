//! The TUI's keys: one flat table that also drives the help overlay and the hint bar.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Decrease,
    Increase,
    Warmer,
    Cooler,
    NextMode,
    NextFan,
    SwingVertical,
    SwingHorizontal,
    Power,
    Send,
    Discard,
    Refresh,
    NextTab,
    /// Jump to the tab with this index.
    Tab(usize),
    Help,
    Quit,
}

/// When a binding is shown in the hint bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Always,
    /// While a unit can be controlled.
    Control,
    /// While changes wait to be sent.
    Pending,
    /// Only in the narrow layout, where the panes are tabs.
    Tabs,
    Never,
}

pub struct Binding {
    /// The keys as the help shows them; empty for a key that shares the line above.
    pub keys: &'static str,
    /// The keys as the hint bar shows them.
    pub short: &'static str,
    pub help: &'static str,
    /// Short label for the hint bar.
    pub label: &'static str,
    pub hint: Hint,
    /// Lower stays longer in the hint bar when the window is narrow.
    pub rank: u8,
    codes: &'static [KeyCode],
    action: Action,
}

/// A second key for the action described on the line above.
const fn also(codes: &'static [KeyCode], action: Action) -> Binding {
    Binding {
        keys: "",
        short: "",
        help: "",
        label: "",
        hint: Hint::Never,
        rank: u8::MAX,
        codes,
        action,
    }
}

use KeyCode::Char;

pub const BINDINGS: &[Binding] = &[
    Binding {
        keys: "↑↓ jk",
        short: "↑↓",
        help: "select a setting",
        label: "select",
        hint: Hint::Control,
        rank: 4,
        codes: &[KeyCode::Up, Char('k')],
        action: Action::Up,
    },
    also(&[KeyCode::Down, Char('j')], Action::Down),
    Binding {
        keys: "←→ hl",
        short: "←→",
        help: "change the selected setting",
        label: "change",
        hint: Hint::Control,
        rank: 4,
        codes: &[KeyCode::Right, Char('l')],
        action: Action::Increase,
    },
    also(&[KeyCode::Left, Char('h')], Action::Decrease),
    Binding {
        keys: "+ -",
        short: "+-",
        help: "target temperature up, down",
        label: "temp",
        hint: Hint::Control,
        rank: 5,
        codes: &[Char('+'), Char('=')],
        action: Action::Warmer,
    },
    also(&[Char('-')], Action::Cooler),
    Binding {
        keys: "m",
        short: "m",
        help: "next mode",
        label: "mode",
        hint: Hint::Control,
        rank: 6,
        codes: &[Char('m')],
        action: Action::NextMode,
    },
    Binding {
        keys: "f",
        short: "f",
        help: "next fan speed",
        label: "fan",
        hint: Hint::Control,
        rank: 6,
        codes: &[Char('f')],
        action: Action::NextFan,
    },
    Binding {
        keys: "s S",
        short: "s",
        help: "swing up and down, left and right",
        label: "swing",
        hint: Hint::Control,
        rank: 6,
        codes: &[Char('s')],
        action: Action::SwingVertical,
    },
    also(&[Char('S')], Action::SwingHorizontal),
    Binding {
        keys: "P",
        short: "P",
        help: "switch on or off",
        label: "power",
        hint: Hint::Control,
        rank: 6,
        codes: &[Char('P')],
        action: Action::Power,
    },
    Binding {
        keys: "⏎",
        short: "⏎",
        help: "send the changes now, without waiting 3 s",
        label: "send now",
        hint: Hint::Pending,
        rank: 0,
        codes: &[KeyCode::Enter],
        action: Action::Send,
    },
    Binding {
        keys: "u",
        short: "u",
        help: "discard the changes",
        label: "discard",
        hint: Hint::Pending,
        rank: 0,
        codes: &[Char('u')],
        action: Action::Discard,
    },
    Binding {
        keys: "r",
        short: "r",
        help: "refresh now if the budget allows, or log in",
        label: "refresh",
        hint: Hint::Always,
        rank: 3,
        codes: &[Char('r')],
        action: Action::Refresh,
    },
    Binding {
        keys: "Tab",
        short: "Tab",
        help: "next tab, in a narrow window",
        label: "next tab",
        hint: Hint::Tabs,
        rank: 3,
        codes: &[KeyCode::Tab],
        action: Action::NextTab,
    },
    Binding {
        keys: "1-4",
        short: "",
        help: "jump to a tab, in a narrow window",
        label: "tab",
        hint: Hint::Never,
        rank: u8::MAX,
        codes: &[Char('1'), Char('2'), Char('3'), Char('4')],
        action: Action::Tab(0),
    },
    Binding {
        keys: "?",
        short: "?",
        help: "this help",
        label: "help",
        hint: Hint::Always,
        rank: 1,
        codes: &[Char('?')],
        action: Action::Help,
    },
    Binding {
        keys: "q  Esc",
        short: "q",
        help: "quit; asks first when changes wait to be sent",
        label: "quit",
        hint: Hint::Always,
        rank: 1,
        codes: &[Char('q'), KeyCode::Esc],
        action: Action::Quit,
    },
];

/// The action bound to `key`. Ctrl-C always quits.
pub fn action(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == Char('c')).then_some(Action::Quit);
    }
    let binding = BINDINGS.iter().find(|b| b.codes.contains(&key.code))?;
    Some(match (binding.action, key.code) {
        (Action::Tab(_), Char(digit)) => {
            let index = digit.to_digit(10).and_then(|d| usize::try_from(d).ok())?;
            Action::Tab(index.checked_sub(1)?)
        }
        (action, _) => action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Option<Action> {
        action(KeyEvent::from(code))
    }

    #[test]
    fn keys_map_to_actions() {
        assert_eq!(press(Char('r')), Some(Action::Refresh));
        assert_eq!(press(Char('3')), Some(Action::Tab(2)));
        assert_eq!(press(KeyCode::Esc), Some(Action::Quit));
        assert_eq!(press(Char('x')), None);
        let ctrl_c = KeyEvent::new(Char('c'), KeyModifiers::CONTROL);
        assert_eq!(action(ctrl_c), Some(Action::Quit));
        let ctrl_r = KeyEvent::new(Char('r'), KeyModifiers::CONTROL);
        assert_eq!(action(ctrl_r), None);
    }

    #[test]
    fn vim_keys_and_arrows_do_the_same() {
        assert_eq!(press(Char('k')), press(KeyCode::Up));
        assert_eq!(press(Char('j')), press(KeyCode::Down));
        assert_eq!(press(Char('h')), press(KeyCode::Left));
        assert_eq!(press(Char('l')), press(KeyCode::Right));
        assert_eq!(press(Char('S')), Some(Action::SwingHorizontal));
    }

    #[test]
    fn every_key_is_bound_once() {
        let mut seen = Vec::new();
        for code in BINDINGS.iter().flat_map(|b| b.codes) {
            assert!(!seen.contains(code), "{code:?} is bound twice");
            seen.push(*code);
        }
    }
}
