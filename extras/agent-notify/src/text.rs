//! Small text helpers shared by the commands.

/// Whether `text` is a whole number written with digits only, as config values and state files
/// must be.
pub fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// 42s, 7m 12s, 1h 05m, 5d 23h.
pub fn duration(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h {:02}m", secs / 3600, secs % 3600 / 60)
    } else {
        format!("{}d {:02}h", secs / 86400, secs % 86400 / 3600)
    }
}

/// Single-quoted for a shell, whatever it holds.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Every run of whitespace as one space.
pub fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out
}

/// Without ASCII control characters, newlines included unless `keep_newlines`.
pub fn strip_controls(text: &str, keep_newlines: bool) -> String {
    text.chars()
        .filter(|&ch| !(ch.is_ascii_control() && !(keep_newlines && ch == '\n')))
        .collect()
}

/// The first `max` characters.
pub fn cut(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// With the characters notification markup gives a meaning to escaped.
pub fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// What follows the last slash, or all of it if there is none.
pub fn last_part(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// One line of text as a command's output gives it: without the newlines it ends with.
pub fn chomp(text: &str) -> &str {
    text.trim_end_matches('\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_in_the_two_largest_units() {
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(60), "1m 00s");
        assert_eq!(duration(432), "7m 12s");
        assert_eq!(duration(3900), "1h 05m");
        assert_eq!(duration(5 * 86400 + 23 * 3600 + 59), "5d 23h");
        assert_eq!(duration(-3), "-3s", "a clock that went back still reads");
    }

    #[test]
    fn a_quoted_word_survives_any_quote_inside_it() {
        assert_eq!(
            shell_quote("/opt/it's here/agent-notify"),
            r"'/opt/it'\''s here/agent-notify'"
        );
    }

    #[test]
    fn whitespace_of_any_kind_collapses_to_one_space() {
        assert_eq!(
            collapse_whitespace("a \t\n b\u{a0}\u{3000}c\u{200b}d "),
            "a b c\u{200b}d "
        );
    }

    #[test]
    fn cutting_counts_characters_not_bytes() {
        assert_eq!(cut("ééé", 2), "éé");
        assert_eq!(cut("ab", 5), "ab");
    }

    #[test]
    fn numbers_are_digits_only() {
        assert!(is_number("120"));
        assert!(!is_number(""));
        assert!(!is_number("-1"));
        assert!(!is_number("12 "));
    }

    #[test]
    fn the_last_part_of_a_path_is_after_its_last_slash() {
        assert_eq!(last_part("/home/robin/kiln"), "kiln");
        assert_eq!(last_part("/home/robin/kiln/"), "");
        assert_eq!(last_part("kiln"), "kiln");
    }
}
