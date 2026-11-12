//! Text from outside (a plugin's output, an app's tray label) made safe to show.

/// Characters that change how the text around them shows without showing themselves: the
/// bidirectional marks, overrides and isolates that turn text round (the Arabic letter mark and
/// the right-to-left override among them), the zero-width characters and the byte order mark.
/// None belongs in a label, and each can make one read as something else.
pub fn hidden(c: char) -> bool {
    matches!(
        c,
        '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}'
            | '\u{feff}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_that_turn_text_round_are_hidden() {
        for c in [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202e}', '\u{2066}', '\u{2069}',
        ] {
            assert!(hidden(c), "{:04x}", c as u32);
        }
        for c in ['\u{200b}', '\u{2060}', '\u{feff}'] {
            assert!(hidden(c), "{:04x}", c as u32);
        }
        for c in ['a', ' ', 'ع', '\u{2070}', '\u{061b}'] {
            assert!(!hidden(c), "{:04x}", c as u32);
        }
    }
}
