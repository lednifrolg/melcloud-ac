//! The TUI's keys: one flat table that also drives the help overlay and the hint bar.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Login,
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
    /// Only in the narrow layout, where the panes are tabs.
    Tabs,
    /// Only while not logged in.
    LoggedOut,
    Never,
}

pub struct Binding {
    /// The keys as the help shows them.
    pub keys: &'static str,
    pub help: &'static str,
    /// Short label for the hint bar.
    pub label: &'static str,
    pub hint: Hint,
    codes: &'static [KeyCode],
    action: Action,
}

pub const BINDINGS: &[Binding] = &[
    Binding {
        keys: "r",
        help: "refresh now, when the request budget allows it",
        label: "refresh",
        hint: Hint::Always,
        codes: &[KeyCode::Char('r')],
        action: Action::Refresh,
    },
    Binding {
        keys: "l",
        help: "log in to MELCloud",
        label: "log in",
        hint: Hint::LoggedOut,
        codes: &[KeyCode::Char('l')],
        action: Action::Login,
    },
    Binding {
        keys: "Tab",
        help: "next tab, in a narrow window",
        label: "next tab",
        hint: Hint::Tabs,
        codes: &[KeyCode::Tab],
        action: Action::NextTab,
    },
    Binding {
        keys: "1-4",
        help: "jump to a tab, in a narrow window",
        label: "tab",
        hint: Hint::Never,
        codes: &[
            KeyCode::Char('1'),
            KeyCode::Char('2'),
            KeyCode::Char('3'),
            KeyCode::Char('4'),
        ],
        action: Action::Tab(0),
    },
    Binding {
        keys: "?",
        help: "this help",
        label: "help",
        hint: Hint::Always,
        codes: &[KeyCode::Char('?')],
        action: Action::Help,
    },
    Binding {
        keys: "q  Esc",
        help: "quit",
        label: "quit",
        hint: Hint::Always,
        codes: &[KeyCode::Char('q'), KeyCode::Esc],
        action: Action::Quit,
    },
];

/// The action bound to `key`. Ctrl-C always quits.
pub fn action(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Action::Quit);
    }
    let binding = BINDINGS.iter().find(|b| b.codes.contains(&key.code))?;
    Some(match (binding.action, key.code) {
        (Action::Tab(_), KeyCode::Char(digit)) => {
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
        assert_eq!(press(KeyCode::Char('r')), Some(Action::Refresh));
        assert_eq!(press(KeyCode::Char('3')), Some(Action::Tab(2)));
        assert_eq!(press(KeyCode::Esc), Some(Action::Quit));
        assert_eq!(press(KeyCode::Char('x')), None);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(action(ctrl_c), Some(Action::Quit));
        let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(action(ctrl_r), None);
    }
}
