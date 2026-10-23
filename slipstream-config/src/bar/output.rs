//! What a module's command prints, read as Waybar reads it.
//!
//! Plain output is the text on the first line, the tooltip on the second and the class on the
//! third. JSON output is an object a line: `{"text": "…", "alt": "…", "tooltip": "…",
//! "class": "…" or ["…"], "percentage": 0–100}`, every field optional.
//!
//! Text and tooltips may carry Pango markup, as they may for Waybar. It is never interpreted: the
//! tags are taken out and the entities decoded ([`plain`]), so a command can't choose a colour, a
//! font or a size. Its class can only pick one of a few tints ([`Tint`]).

use serde_json::Value;

/// The most characters kept of any one thing the command says. The bar shows far fewer.
const KEPT: usize = 1024;

/// The tint a module's class picks. Nothing else about its look is the command's to choose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tint {
    #[default]
    Plain,
    /// `warning`: amber.
    Warning,
    /// `critical` or `urgent`: the bar's hot orange.
    Critical,
}

impl Tint {
    /// The tint of a set of classes: the most pressing of those it knows.
    fn of<'a>(classes: impl IntoIterator<Item = &'a str>) -> Tint {
        classes
            .into_iter()
            .map(|class| match class.trim() {
                "critical" | "urgent" => Tint::Critical,
                "warning" => Tint::Warning,
                _ => Tint::Plain,
            })
            .max_by_key(|tint| *tint as u8)
            .unwrap_or_default()
    }
}

/// One update from a module's command. The text is as it was printed, markup and all, until it
/// is formatted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Output {
    pub text: String,
    pub alt: String,
    pub tooltip: Option<String>,
    pub tint: Tint,
    pub percentage: Option<u8>,
}

impl Output {
    /// Plain output: the text, then the tooltip, then the class, a line each. With no tooltip
    /// line the tooltip is the text, as in Waybar.
    pub fn plain(printed: &str) -> Output {
        let mut lines = printed.lines();
        let text = kept(lines.next().unwrap_or_default());
        let tooltip = lines.next().map(kept).unwrap_or_else(|| text.clone());
        let tint = Tint::of(lines.next());
        Output {
            tooltip: Some(tooltip),
            text,
            tint,
            ..Output::default()
        }
    }

    /// One line of JSON output, or why it isn't one.
    pub fn json(line: &str) -> Result<Output, String> {
        let value: Value = serde_json::from_str(line.trim()).map_err(|err| err.to_string())?;
        let Value::Object(fields) = value else {
            return Err("not a JSON object".into());
        };
        let text = |key: &str| match fields.get(key) {
            Some(Value::String(text)) => Some(kept(text)),
            Some(Value::Number(number)) => Some(number.to_string()),
            Some(Value::Bool(flag)) => Some(flag.to_string()),
            _ => None,
        };
        let tint = match fields.get("class") {
            Some(Value::String(class)) => Tint::of([class.as_str()]),
            Some(Value::Array(classes)) => Tint::of(classes.iter().filter_map(Value::as_str)),
            _ => Tint::Plain,
        };
        let percentage = fields
            .get("percentage")
            .and_then(Value::as_f64)
            .filter(|percent| percent.is_finite())
            .map(|percent| percent.round().clamp(0.0, 100.0) as u8);
        Ok(Output {
            text: text("text").unwrap_or_default(),
            alt: text("alt").unwrap_or_default(),
            tooltip: text("tooltip"),
            tint,
            percentage,
        })
    }

    /// What one run of a command printed, read as `json` says: plain lines, or the last line of
    /// JSON (a command may print one every so often and only the newest counts).
    pub fn of_run(printed: &str, json: bool) -> Result<Output, String> {
        if !json {
            return Ok(Output::plain(printed));
        }
        match printed.lines().rev().find(|line| !line.trim().is_empty()) {
            Some(line) => Output::json(line),
            None => Ok(Output::default()),
        }
    }
}

/// At most `KEPT` characters of `text`.
fn kept(text: &str) -> String {
    text.chars().take(KEPT).collect()
}

/// Pango markup as the plain text it would show, never interpreted: tags are taken out, entities
/// decoded, and control characters (line breaks included, unless `lines`) and the ones that
/// reorder text dropped, so nothing can disguise what it says.
pub fn plain(markup: &str, lines: bool) -> String {
    let mut out = String::with_capacity(markup.len());
    let mut rest = markup;
    while let Some(c) = rest.chars().next() {
        match c {
            // A tag, to its end. A `<` with no `>` after it is only a character.
            '<' => {
                if let Some(end) = rest.find('>') {
                    rest = &rest[end + 1..];
                    continue;
                }
                out.push('<');
            }
            '&' => {
                if let Some((decoded, len)) = entity(rest) {
                    keep(&mut out, decoded, lines);
                    rest = &rest[len..];
                    continue;
                }
                out.push('&');
            }
            c => keep(&mut out, c, lines),
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Adds `c` to `out` unless it's a control character or one that changes the direction of the
/// text around it. A tab or a line break the text can't have becomes a space.
fn keep(out: &mut String, c: char, lines: bool) {
    match c {
        '\n' if lines => out.push('\n'),
        '\t' | '\n' | '\r' => out.push(' '),
        '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {}
        c if c.is_control() => {}
        c => out.push(c),
    }
}

/// The entity `text` starts with, such as `&amp;` or `&#x2603;`, and how many bytes it takes.
fn entity(text: &str) -> Option<(char, usize)> {
    let end = text.bytes().take(12).position(|byte| byte == b';')?;
    let name = &text[1..end];
    let decoded = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((decoded, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_output_is_text_then_tooltip_then_class() {
        assert_eq!(
            Output::plain("21°C\nSunny all day\nwarning\nignored\n"),
            Output {
                text: "21°C".into(),
                tooltip: Some("Sunny all day".into()),
                tint: Tint::Warning,
                ..Output::default()
            }
        );
        let one = Output::plain("42\n");
        assert_eq!(
            one.tooltip.as_deref(),
            Some("42"),
            "the text, as Waybar has it"
        );
        assert_eq!(Output::plain("").text, "");
    }

    #[test]
    fn json_output_reads_every_field_and_minds_none_missing() {
        let output = Output::json(
            r#"{"text": "3 new", "alt": "unread", "tooltip": "From Ada\nFrom Bo",
                "class": ["mail", "urgent"], "percentage": 42.6, "extra": {"x": 1}}"#,
        )
        .unwrap();
        assert_eq!(
            output,
            Output {
                text: "3 new".into(),
                alt: "unread".into(),
                tooltip: Some("From Ada\nFrom Bo".into()),
                tint: Tint::Critical,
                percentage: Some(43),
            }
        );
        assert_eq!(Output::json("{}").unwrap(), Output::default());
        assert_eq!(Output::json(r#"{"text": 7}"#).unwrap().text, "7");
        assert_eq!(
            Output::json(r#"{"class": "warning", "percentage": 400}"#).unwrap(),
            Output {
                tint: Tint::Warning,
                percentage: Some(100),
                ..Output::default()
            }
        );
        assert_eq!(
            Output::json(r#"{"percentage": "high", "class": 3}"#).unwrap(),
            Output::default()
        );
    }

    #[test]
    fn garbage_is_an_error_and_never_a_panic() {
        for garbage in [
            "",
            "nonsense",
            "[1, 2]",
            "\"text\"",
            "{\"text\": ",
            "\u{0}\u{1}{",
        ] {
            assert!(Output::json(garbage).is_err(), "{garbage:?}");
        }
        assert_eq!(Output::of_run("  \n", true), Ok(Output::default()));
        assert_eq!(
            Output::of_run("{\"text\": \"old\"}\n{\"text\": \"new\"}\n", true)
                .unwrap()
                .text,
            "new"
        );
        assert!(Output::of_run("{\"text\": \"a\"}\nnot json\n", true).is_err());
    }

    #[test]
    fn oversize_output_is_cut_down() {
        let long = "x".repeat(10 * KEPT);
        assert_eq!(Output::plain(&long).text.chars().count(), KEPT);
        let json = format!(r#"{{"text": "{long}", "tooltip": "{long}"}}"#);
        let output = Output::json(&json).unwrap();
        assert_eq!(output.text.chars().count(), KEPT);
        assert_eq!(output.tooltip.unwrap().chars().count(), KEPT);
    }

    #[test]
    fn markup_is_shown_as_the_text_it_carries_and_never_obeyed() {
        assert_eq!(
            plain(
                "<span color='#ff0000' font='72'>big</span> <b>red</b>",
                false
            ),
            "big red"
        );
        assert_eq!(
            plain("Tom &amp; Jerry &lt;3 &#9731; &#x1F600;", false),
            "Tom & Jerry <3 ☃ 😀"
        );
        assert_eq!(
            plain("&lt;b&gt;not bold&lt;/b&gt;", false),
            "<b>not bold</b>"
        );
        assert_eq!(
            plain("a < b and c &nonsense; &", false),
            "a < b and c &nonsense; &"
        );
        assert_eq!(plain("one\ntwo\tthree", false), "one two three");
        assert_eq!(plain("one\ntwo", true), "one\ntwo");
        assert_eq!(
            plain("\u{202e}gnp.exe\u{1b}[31m\u{7}", false),
            "gnp.exe[31m",
            "no reordering, escapes or bells"
        );
        assert_eq!(plain("&#0;&#x110000;", false), "&#x110000;");
        assert_eq!(plain("&é;&", false), "&é;&", "never cut inside a character");
    }

    #[test]
    fn a_class_picks_one_of_a_few_tints_and_nothing_else() {
        assert_eq!(Tint::of(["warning"]), Tint::Warning);
        assert_eq!(Tint::of(["critical"]), Tint::Critical);
        assert_eq!(Tint::of(["urgent", "warning"]), Tint::Critical);
        assert_eq!(Tint::of(["#ff0000", "red", "sharing", ""]), Tint::Plain);
    }
}
