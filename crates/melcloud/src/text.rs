//! Text from the server, made safe to show.

/// `text` without control characters, which a terminal could take as commands (escape sequences
/// can set the clipboard or the window title, for example), and without the characters that
/// reverse the direction of the text after them.
pub(crate) fn printable(text: String) -> String {
    if text.chars().any(unsafe_to_show) {
        text.chars().filter(|&c| !unsafe_to_show(c)).collect()
    } else {
        text
    }
}

/// A control character, a line or paragraph separator, or a character that sets the direction
/// of text: marks, embeddings, overrides and isolates.
pub fn unsafe_to_show(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
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
        assert_eq!(
            printable("a\u{9b}31mb\u{202e}c".into()),
            "a31mbc",
            "C1 and bidi"
        );
        assert_eq!(printable("Zoë's café".into()), "Zoë's café");
        assert_eq!(printable("a\u{200f}b\u{2028}c".into()), "abc");
    }
}
