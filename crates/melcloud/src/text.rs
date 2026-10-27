//! Text from the server, made safe to show.

/// `text` without control characters, which a terminal could take as commands: escape sequences
/// can set the clipboard or the window title, for example.
pub(crate) fn printable(text: String) -> String {
    if text.chars().any(char::is_control) {
        text.chars().filter(|c| !c.is_control()).collect()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_are_dropped() {
        assert_eq!(printable("Living room".into()), "Living room");
        assert_eq!(
            printable("Bed\u{1b}]52;c;cHduZWQ=\u{7}room\r\n".into()),
            "Bed]52;c;cHduZWQ=room"
        );
    }
}
